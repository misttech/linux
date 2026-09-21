// SPDX-License-Identifier: GPL-2.0

//! A typed Rust API over `struct hlist_head` and `struct hlist_node`, the
//! singly-linked list with a one-pointer head that hash tables are built on.
//!
//! The list is intrusive: an object embeds an [`hlist_node`], and the list holds
//! nothing else. The nodes are the C structures, laid out exactly, and the C
//! `static inline` functions of `include/linux/list.h` (`hlist_add_head()`,
//! `hlist_del()`, `hlist_for_each_entry()` and the rest) keep working on the same
//! memory. So this API never reasons about who else is on a list, never hands out
//! `&mut` to a node or to an object that embeds one, and leaves the C inlines where
//! they are. It is a header-only unit: there is no C file to replace and no
//! symbol to export.
//!
//! What the type system adds is ownership and a place to stand:
//!
//! - An [`HList`] holds `P`s, owning pointers to objects that embed a node
//!   ([`HlistItem`]). Items go in with `push_front()` and come out as the same
//!   `P`, so the object is freed by whoever owns it, once.
//! - `first->pprev` points into the head, so a head with items on it must not move.
//!   [`HList`] is `!Unpin`, and every operation that links takes
//!   `Pin<&mut HList>`.
//! - An object that is on a list of a `Tag` is refused by the next: `push_front()` and
//!   the inserts give it back. Another reference to the object could offer it again,
//!   and linking a node that is on a list leaves that list pointing at it. The claim of
//!   a node is one compare-exchange, so it holds for two lists under two locks.
//! - Removing an element, or inserting next to one, goes through a [`CursorMut`],
//!   which borrows the list mutably, so no second mutation can happen while it
//!   exists. Removing an element from a list it is not on is the bug this prevents.
//!   What is not prevented is the C side: see [`HList::from_raw()`].
//!
//! This API is for a list that a lock protects, or that only one context touches.
//! The RCU variants (`hlist_add_head_rcu()`, `hlist_del_rcu()`) are not here.
//!
//! The `READ_ONCE()` and `WRITE_ONCE()` of the C are `Relaxed` atomic accesses,
//! and the plain accesses are too: an [`hlist_node`] and an [`hlist_head`] are
//! atomics, so that a plain access from C does not make a shared reference to
//! them undefined.

use core::marker::{PhantomData, PhantomPinned};
use core::mem;
use core::pin::Pin;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::kref::{KRef, KrefCounted};
use crate::port_layout as layout;

/// `struct hlist_node`: the link of an object on a list.
///
/// `pprev` points at whatever points at this node, which is the `first` of the
/// head or the `next` of the node before it, so a node can remove itself without
/// knowing the head.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct hlist_node {
    next: AtomicPtr<hlist_node>,
    pprev: AtomicPtr<AtomicPtr<hlist_node>>,
}

/// `struct hlist_head`: the head of a list, one pointer.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct hlist_head {
    first: AtomicPtr<hlist_node>,
}

// BTF of x86_64 kernels: `hlist_head` is one pointer, and `hlist_node` is two.
// 32-bit values are from DWARF of clang --target=i386 for the same structs
// (include/linux/types.h): sizes 4 and 8, align 4.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(hlist_head, size = 8, align = 8, first @ 0);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(hlist_node, size = 16, align = 8, next @ 0, pprev @ 8);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(hlist_head, size = 4, align = 4, first @ 0);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(hlist_node, size = 8, align = 4, next @ 0, pprev @ 4);

impl hlist_node {
    /// `INIT_HLIST_NODE()`: a node that is on no list.
    pub const fn new() -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            pprev: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// `hlist_unhashed()`: true if the node is on no list.
    pub fn is_unhashed(&self) -> bool {
        self.pprev.load(Ordering::Relaxed).is_null()
    }
}

impl Default for hlist_node {
    fn default() -> Self {
        Self::new()
    }
}

impl hlist_head {
    /// `INIT_HLIST_HEAD()`: an empty head. Nothing points into it yet, so it may
    /// still be moved.
    pub const fn new() -> Self {
        Self {
            first: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// `hlist_empty()`.
    pub fn is_empty(&self) -> bool {
        self.first.load(Ordering::Relaxed).is_null()
    }
}

impl Default for hlist_head {
    fn default() -> Self {
        Self::new()
    }
}

/// `LIST_POISON1`, which `hlist_del()` leaves in `next`. It depends on
/// `CONFIG_ILLEGAL_POINTER_VALUE`, so Kbuild measures it with the C compiler.
const LIST_POISON1: *mut hlist_node = layout::PORT_LIST_POISON1 as *mut hlist_node;

/// `LIST_POISON2`, which `hlist_del()` leaves in `pprev`.
const LIST_POISON2: *mut AtomicPtr<hlist_node> =
    layout::PORT_LIST_POISON2 as *mut AtomicPtr<hlist_node>;

/// The node at `p`.
///
/// # Safety
///
/// `p` is not NULL and points to an `hlist_node` that is live, and that stays
/// live for `'a`: a node on a list that the caller has locked or owns.
unsafe fn node<'a>(p: *mut hlist_node) -> &'a hlist_node {
    // SAFETY: (U3) the caller's contract: `p` is a live node.
    unsafe { &*p }
}

/// The link at `p`: the `first` of a head, or the `next` of a node.
///
/// # Safety
///
/// `p` is not NULL and points to an `AtomicPtr<hlist_node>` that is live: the
/// `pprev` of a node on a list that the caller has locked or owns.
unsafe fn link<'a>(p: *mut AtomicPtr<hlist_node>) -> &'a AtomicPtr<hlist_node> {
    // SAFETY: (U3) the caller's contract: `p` is a live link.
    unsafe { &*p }
}

/// `&n->next`, the address that the node after `n` keeps in its `pprev`. It does not
/// dereference `n`, so the result has the provenance of `n`, the whole object.
fn next_link(n: *mut hlist_node) -> *mut AtomicPtr<hlist_node> {
    n.wrapping_byte_add(mem::offset_of!(hlist_node, next))
        .cast()
}

/// `__hlist_del()`: unlink `n`, leaving its own fields as they were.
///
/// # Safety
///
/// `n` is on a list, and so are the nodes around it; the caller holds whatever
/// protects that list.
unsafe fn hlist_del_raw(n: &hlist_node) {
    let next = n.next.load(Ordering::Relaxed);
    let pprev = n.pprev.load(Ordering::Relaxed);

    // SAFETY: (U3) `n` is on a list, so its `pprev` is the live link that points
    // at it, and `next` is NULL or the live node after it.
    unsafe {
        link(pprev).store(next, Ordering::Relaxed);
        if !next.is_null() {
            node(next).pprev.store(pprev, Ordering::Relaxed);
        }
    }
}

/// `hlist_del_init()`: unlink `n`, and leave it as `INIT_HLIST_NODE()` does, so
/// that it reads as unhashed and may be added again.
///
/// # Safety
///
/// As for [`hlist_del_raw()`], unless `n` is unhashed, in which case nothing happens.
unsafe fn hlist_del_init_raw(n: &hlist_node) {
    if !n.is_unhashed() {
        // SAFETY: (U3) `n` is hashed, so it is on a list, and the caller's contract holds.
        unsafe { hlist_del_raw(n) };
        n.next.store(ptr::null_mut(), Ordering::Relaxed);
        n.pprev.store(ptr::null_mut(), Ordering::Relaxed);
    }
}

/// `hlist_del()`: unlink `n` and poison it. C code and crash dumps rely on the
/// poison, and after it `hlist_unhashed()` is false.
///
/// # Safety
///
/// As for [`hlist_del_raw()`].
unsafe fn hlist_del_poison_raw(n: &hlist_node) {
    // SAFETY: (U3) the caller's contract.
    unsafe { hlist_del_raw(n) };
    n.next.store(LIST_POISON1, Ordering::Relaxed);
    n.pprev.store(LIST_POISON2, Ordering::Relaxed);
}

/// `hlist_add_head()`: put `n` first on the list of `h`.
///
/// # Safety
///
/// `n` points into a live object, and to a node that is on no list and stays where
/// it is while it is on this one, and so does `h`. The caller holds whatever protects
/// the list. `n` is the pointer that came from the object, so it keeps the object's
/// provenance in the list.
unsafe fn hlist_add_head_raw(n: NonNull<hlist_node>, h: &hlist_head) {
    // SAFETY: (U3) the caller's contract: `n` is a live node.
    let node_n = unsafe { node(n.as_ptr()) };
    let first = h.first.load(Ordering::Relaxed);

    node_n.next.store(first, Ordering::Relaxed);
    if !first.is_null() {
        // SAFETY: (U3) `first` is the live node that was first on the list.
        unsafe { node(first) }
            .pprev
            .store(next_link(n.as_ptr()), Ordering::Relaxed);
    }
    h.first.store(n.as_ptr(), Ordering::Relaxed);
    node_n
        .pprev
        .store(ptr::from_ref(&h.first).cast_mut(), Ordering::Relaxed);
}

/// `hlist_add_before()`: put `n` on the list of `next`, just before it.
///
/// # Safety
///
/// `n` points into a live object, and to a node that is on no list and stays where it
/// is, and `next` is a live node on a list. The caller holds whatever protects it.
unsafe fn hlist_add_before_raw(n: NonNull<hlist_node>, next: NonNull<hlist_node>) {
    // SAFETY: (U3) the caller's contract: `n` and `next` are live nodes.
    let (node_n, node_next) = unsafe { (node(n.as_ptr()), node(next.as_ptr())) };

    node_n
        .pprev
        .store(node_next.pprev.load(Ordering::Relaxed), Ordering::Relaxed);
    node_n.next.store(next.as_ptr(), Ordering::Relaxed);
    node_next
        .pprev
        .store(next_link(n.as_ptr()), Ordering::Relaxed);
    // SAFETY: (U3) `n.pprev` is what pointed at `next`, a live link.
    unsafe { link(node_n.pprev.load(Ordering::Relaxed)) }.store(n.as_ptr(), Ordering::Relaxed);
}

/// `hlist_add_behind()`: put `n` on the list of `prev`, just after it.
///
/// # Safety
///
/// `n` points into a live object, and to a node that is on no list and stays where it
/// is, and `prev` is a live node on a list. The caller holds whatever protects it.
unsafe fn hlist_add_behind_raw(n: NonNull<hlist_node>, prev: NonNull<hlist_node>) {
    // SAFETY: (U3) the caller's contract: `n` and `prev` are live nodes.
    let (node_n, node_prev) = unsafe { (node(n.as_ptr()), node(prev.as_ptr())) };

    node_n
        .next
        .store(node_prev.next.load(Ordering::Relaxed), Ordering::Relaxed);
    node_prev.next.store(n.as_ptr(), Ordering::Relaxed);
    node_n
        .pprev
        .store(next_link(prev.as_ptr()), Ordering::Relaxed);
    let after = node_n.next.load(Ordering::Relaxed);
    if !after.is_null() {
        // SAFETY: (U3) `after` is the live node that followed `prev`.
        unsafe { node(after) }
            .pprev
            .store(next_link(n.as_ptr()), Ordering::Relaxed);
    }
}

/// `hlist_move_list()`: move every node of `old` to `new`, which is empty.
///
/// # Safety
///
/// The nodes stay where they are, and so does `new`, while they are on it. The
/// caller holds whatever protects both lists.
unsafe fn hlist_move_list_raw(old: &hlist_head, new: &hlist_head) {
    new.first
        .store(old.first.load(Ordering::Relaxed), Ordering::Relaxed);
    let first = new.first.load(Ordering::Relaxed);
    if !first.is_null() {
        // SAFETY: (U3) `first` is the live node that was first on `old`.
        unsafe { node(first) }
            .pprev
            .store(ptr::from_ref(&new.first).cast_mut(), Ordering::Relaxed);
    }
    old.first.store(ptr::null_mut(), Ordering::Relaxed);
}

/// Says that the field at `OFFSET` of a type is the node of some list. [`impl_has_hlist_node!`]
/// implements it once for each field, so implementing [`HasHlistNode`] twice for one
/// field, under two tags, is a conflicting implementation and does not compile.
#[doc(hidden)]
pub trait FieldUsed<const OFFSET: usize> {}

/// An object that embeds an [`hlist_node`] as a field. `Tag` says which one, for an
/// object that is on more than one list.
///
/// # Safety
///
/// `OFFSET` is the offset of an `hlist_node` field of `Self`, found with
/// `core::mem::offset_of!`, and only the lists of that `Tag` use that field. The
/// [`impl_has_hlist_node!`] macro writes this impl and checks the field's type.
/// The node is initialized (`INIT_HLIST_NODE()`, [`hlist_node::new()`] or zeroes)
/// before its object is first given to a list.
pub unsafe trait HasHlistNode<Tag = ()> {
    /// The offset of the embedded `hlist_node`.
    const OFFSET: usize;
}

/// Implement [`HasHlistNode`] for a field of a type, which must not be generic, and
/// which the macro may name for one tag only.
///
/// `impl_has_hlist_node!(Conn, node)` is for the one list an object is on, and
/// `impl_has_hlist_node!(Conn, by_id, ById)` for the list of the tag type `ById`.
#[macro_export]
macro_rules! impl_has_hlist_node {
    ($ty:ty, $field:ident) => {
        $crate::impl_has_hlist_node!($ty, $field, ());
    };
    ($ty:ty, $field:ident, $tag:ty) => {
        // SAFETY: (U2) the offset is that of the field, and the field is checked
        // to be an hlist_node below.
        unsafe impl $crate::hlist::HasHlistNode<$tag> for $ty {
            const OFFSET: usize = ::core::mem::offset_of!($ty, $field);
        }
        impl $crate::hlist::FieldUsed<{ ::core::mem::offset_of!($ty, $field) }> for $ty {}
        const _: fn(&$ty) -> &$crate::hlist::hlist_node = |x| &x.$field;
    };
}

/// An owning pointer to a [`HasHlistNode`] object, which a list can hold.
///
/// # Safety
///
/// [`into_raw()`](Self::into_raw) gives up one owner of the object, and the object
/// stays valid until [`from_raw()`](Self::from_raw) takes that owner back. Nothing
/// but a list uses the node of that `Tag` in between.
pub unsafe trait HlistItem<Tag = ()>: Sized {
    /// The object the pointer owns.
    type Target: HasHlistNode<Tag>;

    /// Gives up ownership of the object.
    fn into_raw(self) -> NonNull<Self::Target>;

    /// Takes back an object that [`into_raw()`](Self::into_raw) gave up.
    ///
    /// # Safety
    ///
    /// `ptr` comes from `into_raw()`, is not on a list, and is taken back only once.
    unsafe fn from_raw(ptr: NonNull<Self::Target>) -> Self;
}

// SAFETY: (U4) a KRef owns one reference, which into_raw() hands to the list and
// from_raw() takes back; the object stays live while the reference is held.
unsafe impl<T: KrefCounted + HasHlistNode<Tag>, Tag> HlistItem<Tag> for KRef<T> {
    type Target = T;

    fn into_raw(self) -> NonNull<T> {
        KRef::into_raw(self)
    }

    unsafe fn from_raw(ptr: NonNull<T>) -> Self {
        // SAFETY: (U4) the caller's contract is the one of KRef::from_raw().
        unsafe { KRef::from_raw(ptr) }
    }
}

/// What the `pprev` of a node holds between the moment a list claims the node and the
/// moment it links it: not NULL, and not `LIST_POISON2`, so that nothing else claims it.
const CLAIMED: *mut AtomicPtr<hlist_node> = ptr::dangling_mut();

/// The `pprev` of a node, as [`claim_link()`] uses it. This is the seam that lets the
/// harness run the claim on loom's atomics.
pub(crate) trait PprevCell {
    /// The link.
    fn load(&self) -> *mut AtomicPtr<hlist_node>;

    /// Replaces `current` by `new`, and says whether it did.
    fn compare_exchange(
        &self,
        current: *mut AtomicPtr<hlist_node>,
        new: *mut AtomicPtr<hlist_node>,
    ) -> bool;
}

impl PprevCell for AtomicPtr<AtomicPtr<hlist_node>> {
    fn load(&self) -> *mut AtomicPtr<hlist_node> {
        AtomicPtr::load(self, Ordering::Relaxed)
    }

    fn compare_exchange(
        &self,
        current: *mut AtomicPtr<hlist_node>,
        new: *mut AtomicPtr<hlist_node>,
    ) -> bool {
        AtomicPtr::compare_exchange(self, current, new, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

/// Claims a node for a list: if it is on no list, unhashed or left poisoned by
/// `hlist_del()`, marks it as not free and returns true, and otherwise returns false.
///
/// The check and the mark are one compare-exchange, because a caller with two lists,
/// each under a lock of its own, and two references to one object can offer the object
/// to both at once. Linking a node that is on a list leaves that list pointing at it,
/// and the owner of the object may free it.
pub(crate) fn claim_link(pprev: &impl PprevCell) -> bool {
    let current = pprev.load();

    (current.is_null() || current == LIST_POISON2) && pprev.compare_exchange(current, CLAIMED)
}

/// Takes the node of `item` for a list, if it is on no list, or gives `item` back.
fn claim<P: HlistItem<Tag>, Tag>(item: P) -> Result<NonNull<hlist_node>, P> {
    let obj = item.into_raw();
    let n = node_of::<_, Tag>(obj);
    // SAFETY: (U3) `obj` is live until from_raw() takes the owner back, and the node
    // is inside it.
    let node_n = unsafe { node(n.as_ptr()) };

    if claim_link(&node_n.pprev) {
        Ok(n)
    } else {
        // SAFETY: (U4) `obj` just came from into_raw(), and no list holds it.
        Err(unsafe { P::from_raw(obj) })
    }
}

/// The node embedded in `obj`.
fn node_of<T: HasHlistNode<Tag>, Tag>(obj: NonNull<T>) -> NonNull<hlist_node> {
    // An address inside an object is not NULL, so the fallback is never taken.
    NonNull::new(obj.as_ptr().wrapping_byte_add(T::OFFSET).cast()).unwrap_or(NonNull::dangling())
}

/// The object that embeds `n`: `hlist_entry()`.
fn entry_of<T: HasHlistNode<Tag>, Tag>(n: NonNull<hlist_node>) -> NonNull<T> {
    // A node found on a list is inside an object, so this is not NULL.
    NonNull::new(n.as_ptr().wrapping_byte_sub(T::OFFSET).cast()).unwrap_or(NonNull::dangling())
}

/// A list of `P`s, over an `hlist_head`.
///
/// A head with items on it must not move, because the first node's `pprev` points
/// into it, so an `HList` is `!Unpin` and every operation that changes it takes
/// `Pin<&mut Self>`. Build one in place with `Box::pin()` or `core::pin::pin!()`,
/// or view a head that C owns with [`from_raw()`](Self::from_raw). An empty list
/// may be built anywhere, and moved until it is pinned.
///
/// Dropping a list that owns items drops the items.
#[repr(transparent)]
pub struct HList<P: HlistItem<Tag>, Tag = ()> {
    head: hlist_head,
    /// Owns `P`s, and lends `P::Target`s to whoever shares the list, so the auto
    /// traits follow both.
    _items: PhantomData<(P, P::Target, Tag)>,
    _pin: PhantomPinned,
}

impl<P: HlistItem<Tag>, Tag> HList<P, Tag> {
    /// An empty list: `INIT_HLIST_HEAD()`.
    pub const fn new() -> Self {
        Self {
            head: hlist_head::new(),
            _items: PhantomData,
            _pin: PhantomPinned,
        }
    }

    /// View a head that C owns, or that lives in an array or a structure, as a list.
    ///
    /// # Safety
    ///
    /// `head` points to an initialized `hlist_head` that stays where it is for `'a`,
    /// and whose nodes are all in objects that `P` can take back. Nothing else
    /// reads or writes the list while the reference lives: the caller holds its lock,
    /// or C does not touch it. Dropping the reference does not drop the items.
    pub unsafe fn from_raw<'a>(head: *mut hlist_head) -> Pin<&'a mut Self> {
        // HList is repr(transparent) over hlist_head, so the cast keeps the layout.
        let list = head.cast::<Self>();
        // SAFETY: (U3) the caller's contract: the head is live and not aliased.
        let list = unsafe { &mut *list };

        // SAFETY: (U7) the caller's contract: the head is where its owner keeps it, and
        // stays there. No class of the taxonomy fits a pin projection; pinning is the
        // closest, and the record proposes one.
        unsafe { Pin::new_unchecked(list) }
    }

    /// The `i`th list of a pinned slice of lists: a hash table's buckets.
    pub fn get_mut(lists: Pin<&mut [Self]>, i: usize) -> Option<Pin<&mut Self>> {
        // SAFETY: (U7) pinning: the slice is pinned, so its elements stay put, and this
        // only borrows one, which is pinned again below. No class of the taxonomy fits
        // a pin projection; the record proposes one.
        unsafe { lists.get_unchecked_mut() }
            .get_mut(i)
            // SAFETY: (U7) as above: an element of a pinned slice is pinned.
            .map(|list| unsafe { Pin::new_unchecked(list) })
    }

    /// `hlist_empty()`.
    pub fn is_empty(&self) -> bool {
        self.head.is_empty()
    }

    /// `hlist_add_head()`: put `item` first. Gives the item back if its node is on a
    /// list already, which another reference to the object can cause.
    pub fn push_front(self: Pin<&mut Self>, item: P) -> Result<(), P> {
        let n = claim::<P, Tag>(item)?;

        // SAFETY: (U3) `n` is in a live object that the list owns from here, and is on
        // no list. This list is pinned, so the head stays where it is.
        unsafe { hlist_add_head_raw(n, &self.head) };
        Ok(())
    }

    /// `hlist_del_init()` of the first item: take it off the list, and give it back.
    pub fn pop_front(self: Pin<&mut Self>) -> Option<P> {
        self.pop()
    }

    /// `pop_front()` without the pin: taking a node off moves no head, so a list that is
    /// being dropped can do it too.
    fn pop(&self) -> Option<P> {
        let first = NonNull::new(self.head.first.load(Ordering::Relaxed))?;

        // SAFETY: (U3) `first` is on this list, which the list owns.
        unsafe { hlist_del_init_raw(node(first.as_ptr())) };
        // SAFETY: (U4) `first` is the node of an object that push_front() or an
        // insert gave up with into_raw(), and it just left the list.
        Some(unsafe { P::from_raw(entry_of::<P::Target, Tag>(first)) })
    }

    /// `hlist_move_list()`: move every item of `other` to this list, which must be
    /// empty.
    pub fn take_from(self: Pin<&mut Self>, other: Pin<&mut Self>) -> bool {
        if !self.is_empty() {
            return false;
        }
        // SAFETY: (U3) both lists are pinned, so neither head moves, and the nodes
        // on `other` are the list's own.
        unsafe { hlist_move_list_raw(&other.head, &self.head) };
        true
    }

    /// The objects on the list, first to last. Removing one takes a cursor.
    pub fn iter(&self) -> Iter<'_, P::Target, Tag> {
        Iter {
            next: self.head.first.load(Ordering::Relaxed),
            _list: PhantomData,
        }
    }

    /// A cursor at the first item.
    pub fn cursor_front_mut(self: Pin<&mut Self>) -> CursorMut<'_, P, Tag> {
        let cur = self.head.first.load(Ordering::Relaxed);
        CursorMut { _list: self, cur }
    }
}

impl<P: HlistItem<Tag>, Tag> Default for HList<P, Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: HlistItem<Tag>, Tag> Drop for HList<P, Tag> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

/// The objects of an [`HList`], first to last.
pub struct Iter<'a, T: HasHlistNode<Tag>, Tag = ()> {
    next: *mut hlist_node,
    _list: PhantomData<(&'a T, Tag)>,
}

impl<'a, T: HasHlistNode<Tag>, Tag> Iterator for Iter<'a, T, Tag> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        let n = NonNull::new(self.next)?;

        // SAFETY: (U3) `n` is on the list that this iterator borrows, so it is live
        // and its object is too.
        self.next = unsafe { node(n.as_ptr()) }.next.load(Ordering::Relaxed);
        // SAFETY: (U3) `n` is inside a live object of type `T`, which the list owns.
        Some(unsafe { entry_of::<T, Tag>(n).as_ref() })
    }
}

/// A position in an [`HList`], which borrows it mutably: nothing else can change
/// the list while the cursor exists. The cursor is at an item, or past the end.
pub struct CursorMut<'a, P: HlistItem<Tag>, Tag = ()> {
    /// Holds the borrow of the list for as long as the cursor lives.
    _list: Pin<&'a mut HList<P, Tag>>,
    cur: *mut hlist_node,
}

impl<P: HlistItem<Tag>, Tag> CursorMut<'_, P, Tag> {
    /// The object at the cursor, or `None` past the end.
    pub fn current(&self) -> Option<&P::Target> {
        let n = NonNull::new(self.cur)?;

        // SAFETY: (U3) the cursor's node is on the list, which the cursor borrows.
        Some(unsafe { entry_of::<P::Target, Tag>(n).as_ref() })
    }

    /// Move to the next item, or past the end.
    pub fn move_next(&mut self) {
        if let Some(n) = NonNull::new(self.cur) {
            // SAFETY: (U3) as current().
            self.cur = unsafe { node(n.as_ptr()) }.next.load(Ordering::Relaxed);
        }
    }

    /// `hlist_del_init()`: take the item at the cursor off the list, give it back,
    /// and move to the next.
    pub fn remove_current(&mut self) -> Option<P> {
        let n = NonNull::new(self.cur)?;

        // SAFETY: (U3) as current(), and the node is on this list.
        let n_ref = unsafe { node(n.as_ptr()) };
        self.cur = n_ref.next.load(Ordering::Relaxed);
        // SAFETY: (U3) as above.
        unsafe { hlist_del_init_raw(n_ref) };
        // SAFETY: (U4) the node is that of an object the list owns, and just left it.
        Some(unsafe { P::from_raw(entry_of::<P::Target, Tag>(n)) })
    }

    /// `hlist_del()`: as [`remove_current()`](Self::remove_current), but leave the
    /// node poisoned, as C does, for an object that lives on after the list.
    pub fn remove_current_poisoned(&mut self) -> Option<P> {
        let n = NonNull::new(self.cur)?;

        // SAFETY: (U3) as remove_current().
        let n_ref = unsafe { node(n.as_ptr()) };
        self.cur = n_ref.next.load(Ordering::Relaxed);
        // SAFETY: (U3) as above.
        unsafe { hlist_del_poison_raw(n_ref) };
        // SAFETY: (U4) as remove_current().
        Some(unsafe { P::from_raw(entry_of::<P::Target, Tag>(n)) })
    }

    /// `hlist_add_before()`: put `item` before the item at the cursor. Gives the item
    /// back if the cursor is past the end, or its node is on a list already.
    pub fn insert_before(&mut self, item: P) -> Result<(), P> {
        let Some(at) = NonNull::new(self.cur) else {
            return Err(item);
        };
        let n = claim::<P, Tag>(item)?;

        // SAFETY: (U3) `n` is in a live object that the list owns from here, and is on
        // no list, and `at` is on this list.
        unsafe { hlist_add_before_raw(n, at) };
        Ok(())
    }

    /// `hlist_add_behind()`: put `item` after the item at the cursor. Gives the item
    /// back if the cursor is past the end, or its node is on a list already.
    pub fn insert_after(&mut self, item: P) -> Result<(), P> {
        let Some(at) = NonNull::new(self.cur) else {
            return Err(item);
        };
        let n = claim::<P, Tag>(item)?;

        // SAFETY: (U3) as insert_before().
        unsafe { hlist_add_behind_raw(n, at) };
        Ok(())
    }
}
