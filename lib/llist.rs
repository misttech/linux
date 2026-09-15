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

use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

/// Mirror of `struct llist_node`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct llist_node {
    next: AtomicPtr<llist_node>,
}

/// Mirror of `struct llist_head`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct llist_head {
    first: AtomicPtr<llist_node>,
}

// BTF of x86_64 kernels: one pointer each. 32-bit values are the pointer size,
// not yet checked against a 32-bit kernel's BTF.
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
}

// The orderings follow the kernel's: smp_load_acquire() is Acquire,
// try_cmpxchg() is fully ordered (SeqCst), and READ_ONCE() and plain assignments
// carry no LKMM ordering (Relaxed).
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
