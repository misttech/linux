// SPDX-License-Identifier: GPL-2.0-only

//! Lock-less NULL terminated single linked list.
//!
//! The Rust implementation of `lib/llist.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/llist.h` is the ABI contract, and its
//! static inlines stay in C. `lib/llist_ffi.c` exports these symbols with the
//! license of `lib/llist.c`.
//!
//! The basic atomic operation of this list is cmpxchg on long. On
//! architectures that don't have NMI-safe cmpxchg implementation, the list can
//! NOT be used in NMI handlers. So code that uses the list in an NMI handler
//! should depend on CONFIG_ARCH_HAVE_NMI_SAFE_CMPXCHG.
//!
//! Rust callers use [`LList`], which encodes the locking table of
//! `include/linux/llist.h` in types: adds and [`LList::del_all()`] are
//! lock-less from any context, while `llist_del_first()` is only reachable
//! through the one [`Consumer`] of a list, which excludes every other deleter.
//! The list does not own its nodes: it holds [`ListItem`] pointers, and hands
//! them back when they are deleted.

use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering};

/// Mirror of `struct llist_node`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct llist_node {
    next: AtomicPtr<llist_node>,
}

impl llist_node {
    /// A node that is not on any list yet.
    pub const fn new() -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

impl Default for llist_node {
    fn default() -> Self {
        Self::new()
    }
}

/// Mirror of `struct llist_head`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct llist_head {
    first: AtomicPtr<llist_node>,
}

// BTF of x86_64 kernels: one pointer each. 32-bit values are from pahole of
// gcc -m32 DWARF of the same structs (include/linux/llist.h): size 4, align 4,
// the pointer at offset 0.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(llist_node, size = 8, align = 8, next @ 0);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(llist_head, size = 8, align = 8, first @ 0);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(llist_node, size = 4, align = 4, next @ 0);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(llist_head, size = 4, align = 4, first @ 0);

/// The atomic pointer operations the list uses.
///
/// Implemented here for `AtomicPtr`. The harness implements it for loom's
/// atomic, so its model runs the code in this file.
pub(crate) trait AtomicLink<T> {
    /// `smp_load_acquire()`.
    fn load_acquire(&self) -> *mut T;

    /// `READ_ONCE()`, or a plain load.
    fn read_once(&self) -> *mut T;

    /// A plain store, as `new_last->next = first` in `llist_add_batch()`.
    fn write(&self, new: *mut T);

    /// `try_cmpxchg()`: on failure, `old` gets the current value.
    fn try_cmpxchg(&self, old: &mut *mut T, new: *mut T) -> bool;

    /// `xchg()`.
    fn xchg(&self, new: *mut T) -> *mut T;
}

// The orderings follow the kernel's: smp_load_acquire() is Acquire,
// try_cmpxchg() and xchg() are fully ordered (SeqCst), and READ_ONCE() and plain
// assignments carry no LKMM ordering (Relaxed).
impl<T> AtomicLink<T> for AtomicPtr<T> {
    #[inline]
    fn load_acquire(&self) -> *mut T {
        self.load(Ordering::Acquire)
    }

    #[inline]
    fn read_once(&self) -> *mut T {
        self.load(Ordering::Relaxed)
    }

    #[inline]
    fn write(&self, new: *mut T) {
        AtomicPtr::store(self, new, Ordering::Relaxed);
    }

    #[inline]
    fn try_cmpxchg(&self, old: &mut *mut T, new: *mut T) -> bool {
        match self.compare_exchange(*old, new, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => true,
            Err(current) => {
                *old = current;
                false
            }
        }
    }

    #[inline]
    fn xchg(&self, new: *mut T) -> *mut T {
        self.swap(new, Ordering::SeqCst)
    }
}

/// The body of `llist_add_batch()` in `include/linux/llist.h`, generic over
/// the atomic.
///
/// `new_last_next` is the `next` link of the last entry of the batch. Returns
/// whether the list was empty before adding.
pub(crate) fn add_batch<T>(
    head: &impl AtomicLink<T>,
    new_first: *mut T,
    new_last_next: &impl AtomicLink<T>,
) -> bool {
    let mut first = head.read_once();

    loop {
        new_last_next.write(first);
        if head.try_cmpxchg(&mut first, new_first) {
            return first.is_null();
        }
    }
}

/// The body of `llist_del_all()` in `include/linux/llist.h`, generic over the
/// atomic.
pub(crate) fn del_all<T>(head: &impl AtomicLink<T>) -> *mut T {
    head.xchg(ptr::null_mut())
}

/// The body of [`llist_del_first()`], generic over the atomic.
///
/// `next` reads the `next` link of an entry on the list.
pub(crate) fn del_first<T>(head: &impl AtomicLink<T>, next: impl Fn(*mut T) -> *mut T) -> *mut T {
    let mut entry = head.load_acquire();

    loop {
        if entry.is_null() {
            return ptr::null_mut();
        }
        let new_first = next(entry);
        if head.try_cmpxchg(&mut entry, new_first) {
            return entry;
        }
    }
}

/// The body of [`llist_del_first_this()`], generic over the atomic.
///
/// `next` reads the `next` link of an entry on the list.
pub(crate) fn del_first_this<T>(
    head: &impl AtomicLink<T>,
    this: *mut T,
    next: impl Fn(*mut T) -> *mut T,
) -> bool {
    // acquire ensures orderig wrt try_cmpxchg() is llist_del_first()
    let mut entry = head.load_acquire();

    loop {
        if entry != this {
            return false;
        }
        let new_first = next(entry);
        if head.try_cmpxchg(&mut entry, new_first) {
            return true;
        }
    }
}

/// `llist_del_first()`: deletes the first entry of lock-less list.
///
/// If list is empty, return NULL, otherwise, return the first entry deleted,
/// this is the newest added one.
///
/// Only one llist_del_first user can be used simultaneously with multiple
/// llist_add users without lock. Because otherwise llist_del_first, llist_add,
/// llist_add (or llist_del_all, llist_add, llist_add) sequence in another user
/// may change `head->first->next`, but keep `head->first`. If multiple
/// consumers are needed, please use llist_del_all or use lock between
/// consumers.
///
/// # Safety
///
/// `head` points to a live `llist_head` whose entries are live, and the caller
/// is its only deleter for the duration of the call, as above.
#[no_mangle]
pub unsafe extern "C" fn llist_del_first(head: *mut llist_head) -> *mut llist_node {
    // SAFETY: (U3) head points to a live llist_head, per the C contract.
    let head = unsafe { &*head };

    del_first(&head.first, |entry| {
        // SAFETY: (U3) entry is on the list, and the caller is its only
        // deleter, so nobody frees entry while its next link is read.
        unsafe { (*entry).next.read_once() }
    })
}

/// `llist_del_first_this()`: deletes the given entry of lock-less list if it is
/// first.
///
/// If head of the list is given entry, delete and return true, else return
/// false.
///
/// Multiple callers can safely call this concurrently with multiple
/// llist_add() callers, providing all the callers offer a different `this`.
///
/// # Safety
///
/// `head` points to a live `llist_head` whose entries are live, and no other
/// caller offers the same `this` concurrently.
#[no_mangle]
pub unsafe extern "C" fn llist_del_first_this(
    head: *mut llist_head,
    this: *mut llist_node,
) -> bool {
    // SAFETY: (U3) head points to a live llist_head, per the C contract.
    let head = unsafe { &*head };

    del_first_this(&head.first, this, |entry| {
        // SAFETY: (U3) entry is this, which the caller keeps alive and which no
        // other caller deletes, so its next link can be read.
        unsafe { (*entry).next.read_once() }
    })
}

/// `llist_reverse_order()`: reverses order of a llist chain.
///
/// Reverse the order of a chain of llist entries and return the new first
/// entry.
///
/// # Safety
///
/// `head` is NULL or the first entry of a chain that was deleted from its list,
/// which the caller owns.
#[no_mangle]
pub unsafe extern "C" fn llist_reverse_order(head: *mut llist_node) -> *mut llist_node {
    let mut head = head;
    let mut new_head = ptr::null_mut();

    while !head.is_null() {
        let tmp = head;
        // SAFETY: (U3) tmp is an entry of the deleted chain, which the caller
        // owns, so nothing else reads or writes its next link.
        let tmp_node = unsafe { &*tmp };
        head = tmp_node.next.read_once();
        tmp_node.next.write(new_head);
        new_head = tmp;
    }

    new_head
}

/// An object that embeds an `llist_node`.
///
/// # Safety
///
/// `OFFSET` is the offset of an `llist_node` field of `Self`, found with
/// `core::mem::offset_of!`, and only the lists the object is added to use
/// that field.
pub unsafe trait HasLlistNode {
    /// The offset of the embedded `llist_node`.
    const OFFSET: usize;
}

/// An owning pointer to a [`HasLlistNode`] object, which a list can hold.
///
/// # Safety
///
/// [`into_raw()`](Self::into_raw) gives up the only owner of the object, which
/// stays valid, and whose `llist_node` nothing but a list uses, until
/// [`from_raw()`](Self::from_raw) takes it back.
pub unsafe trait ListItem: Sized {
    /// The object the pointer owns.
    type Target: HasLlistNode;

    /// Gives up ownership of the object.
    fn into_raw(self) -> NonNull<Self::Target>;

    /// Takes back an object that [`into_raw()`](Self::into_raw) gave up.
    ///
    /// # Safety
    ///
    /// `ptr` comes from `into_raw()`, and is taken back only once.
    unsafe fn from_raw(ptr: NonNull<Self::Target>) -> Self;
}

/// The `llist_node` embedded in `obj`.
fn node_of<T: HasLlistNode>(obj: NonNull<T>) -> *mut llist_node {
    obj.as_ptr().wrapping_byte_add(T::OFFSET).cast()
}

/// The object that embeds `node`: `llist_entry()`.
fn entry_of<T: HasLlistNode>(node: NonNull<llist_node>) -> Option<NonNull<T>> {
    NonNull::new(node.as_ptr().wrapping_byte_sub(T::OFFSET).cast())
}

/// A lock-less list of `P`s, over an `llist_head`.
///
/// Any number of threads may [`add()`](Self::add) and
/// [`del_all()`](Self::del_all) at once. `llist_del_first()` needs a single
/// deleter, so it is only available through the [`Consumer`] that
/// [`split()`](Self::split) returns, while the list is borrowed exclusively.
#[repr(transparent)]
pub struct LList<P: ListItem> {
    head: llist_head,
    _items: PhantomData<P>,
}

// SAFETY: (U6) the list hands Ps from the threads that add them to the threads
// that delete them, so it may move between threads when P: Send.
unsafe impl<P: ListItem + Send> Send for LList<P> {}

// SAFETY: (U6) &LList allows add() and del_all() from many threads at once,
// which the lock-less cmpxchg and xchg on head.first make safe, and which only
// move Ps between threads.
unsafe impl<P: ListItem + Send> Sync for LList<P> {}

impl<P: ListItem> LList<P> {
    /// `LLIST_HEAD_INIT()`: an empty list.
    pub const fn new() -> Self {
        Self {
            head: llist_head {
                first: AtomicPtr::new(ptr::null_mut()),
            },
            _items: PhantomData,
        }
    }

    /// `llist_add()`: adds an item, lock-less, from any context.
    ///
    /// Returns true if the list was empty prior to adding this entry.
    pub fn add(&self, item: P) -> bool {
        let node = node_of(item.into_raw());
        // SAFETY: (U3) into_raw() gave up the only owner of the object, so its
        // llist_node is valid and only this list uses it until it is deleted.
        let links = unsafe { &(*node).next };

        add_batch(&self.head.first, node, links)
    }

    /// `llist_del_all()`: deletes all items, lock-less, from any context.
    ///
    /// The items come out from the newest to the oldest added one.
    pub fn del_all(&self) -> Drain<P> {
        Drain {
            next: del_all(&self.head.first),
            _items: PhantomData,
        }
    }

    /// `llist_empty()`: whether the list is empty.
    ///
    /// Not guaranteed to be accurate or up to date.
    pub fn is_empty(&self) -> bool {
        self.head.first.read_once().is_null()
    }

    /// Splits the list into producers and its one consumer, for as long as they
    /// borrow it.
    pub fn split(&mut self) -> (Producer<'_, P>, Consumer<'_, P>) {
        let list = &*self;

        (Producer { list }, Consumer { list })
    }
}

impl<P: ListItem> Default for LList<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: ListItem> Drop for LList<P> {
    fn drop(&mut self) {
        drop(self.del_all());
    }
}

/// Adds items to a list whose [`Consumer`] exists.
pub struct Producer<'a, P: ListItem> {
    list: &'a LList<P>,
}

impl<P: ListItem> Clone for Producer<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ListItem> Copy for Producer<'_, P> {}

impl<P: ListItem> Producer<'_, P> {
    /// `llist_add()`: adds an item, lock-less, from any context.
    ///
    /// Returns true if the list was empty prior to adding this entry.
    pub fn add(&self, item: P) -> bool {
        self.list.add(item)
    }
}

/// The only deleter of a list, for as long as it exists.
pub struct Consumer<'a, P: ListItem> {
    list: &'a LList<P>,
}

impl<P: ListItem> Consumer<'_, P> {
    /// `llist_del_first()`: deletes the newest item.
    pub fn del_first(&mut self) -> Option<P> {
        let node = del_first(&self.list.head.first, |entry: *mut llist_node| {
            // SAFETY: (U3) entry is on the list, so its object stays valid, and
            // this consumer is the list's only deleter, so nobody deletes it
            // while its next link is read.
            unsafe { (*entry).next.read_once() }
        });
        let obj = entry_of(NonNull::new(node)?)?;

        // SAFETY: (U3) the object was added through add(), from into_raw(), and
        // deleting it from the list hands it back once.
        Some(unsafe { P::from_raw(obj) })
    }

    /// `llist_del_all()`: deletes all items, newest first.
    pub fn del_all(&mut self) -> Drain<P> {
        self.list.del_all()
    }
}

/// Items deleted from a list, as a chain that the `Drain` owns.
///
/// Iterating gives the items back, from the newest to the oldest added one;
/// dropping the `Drain` drops the rest.
pub struct Drain<P: ListItem> {
    next: *mut llist_node,
    _items: PhantomData<P>,
}

// SAFETY: (U6) the chain is detached from its list and owned by the Drain, so
// moving it moves only the Ps.
unsafe impl<P: ListItem + Send> Send for Drain<P> {}

impl<P: ListItem> Drain<P> {
    /// `llist_reverse_order()`: the same items, from the oldest to the newest.
    pub fn reversed(self) -> Self {
        let this = ManuallyDrop::new(self);

        Self {
            // SAFETY: (U3) the chain was deleted from its list and this Drain
            // owns it, as llist_reverse_order() requires.
            next: unsafe { llist_reverse_order(this.next) },
            _items: PhantomData,
        }
    }
}

impl<P: ListItem> Iterator for Drain<P> {
    type Item = P;

    fn next(&mut self) -> Option<P> {
        let node = NonNull::new(self.next)?;

        // SAFETY: (U3) node is on the deleted chain, which this Drain owns.
        self.next = unsafe { node.as_ref() }.next.read_once();
        let obj = entry_of(node)?;

        // SAFETY: (U3) the object was added through add(), from into_raw(), and
        // leaving the chain hands it back once.
        Some(unsafe { P::from_raw(obj) })
    }
}

impl<P: ListItem> Drop for Drain<P> {
    fn drop(&mut self) {
        for item in self.by_ref() {
            drop(item);
        }
    }
}
