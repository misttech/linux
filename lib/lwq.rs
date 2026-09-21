// SPDX-License-Identifier: GPL-2.0

//! Light-weight single-linked queue, replacing `lib/lwq.c` in place.
//!
//! Entries are enqueued to the head of an llist, with no blocking. This can
//! happen in any context, and it is C code in `include/linux/lwq.h`.
//!
//! Entries are dequeued using a spinlock to protect against multiple access.
//! The llist is staged in reverse order, and refreshed from the llist when it
//! exhausts.
//!
//! This is particularly suitable when work items are queued in BH or IRQ
//! context, and where work items are handled one at a time by dedicated
//! threads.
//!
//! `struct lwq` embeds a `spinlock_t`, whose size depends on the configuration
//! (0 bytes on UP, 4 on SMP, more with lock debugging, and an `rt_mutex` on
//! `PREEMPT_RT`), so the offsets of the two fields after it move. Kbuild
//! measures the layout with the C compiler (`kernel/port-layout.c`) and the
//! mirror below asserts exactly those values, with the lock kept opaque.
//!
//! The algorithm is generic over [`Lwq`], which the live C state implements
//! here. The harness implements it on loom's atomics, so its models run the
//! code of this file rather than a copy of it.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::llist::{self, llist_head, llist_node, AtomicLink};
use crate::port_layout as layout;

/// `struct lwq` from `include/linux/lwq.h`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct lwq {
    /// `spinlock_t`, opaque: its size is the only thing Rust needs.
    #[allow(dead_code)]
    lock: kr::OpaqueBytes<{ layout::PORT_LWQ_LOCK_SIZE }>,
    /// Entries to be dequeued.
    ready: AtomicPtr<llist_node>,
    /// Entries being enqueued.
    new: llist_head,
}

kr::static_assert_layout!(
    lwq,
    size = layout::PORT_LWQ_SIZE,
    align = layout::PORT_LWQ_ALIGN,
    lock @ layout::PORT_LWQ_LOCK,
    ready @ layout::PORT_LWQ_READY,
    new @ layout::PORT_LWQ_NEW,
);

unsafe extern "C" {
    /// `lwq_empty()`, a static inline of `<linux/lwq.h>`.
    fn c_lwq_empty(q: *mut c_void) -> bool;
    /// `spin_lock(&q->lock)`.
    fn c_lwq_spin_lock(q: *mut c_void);
    /// `spin_unlock(&q->lock)`.
    fn c_lwq_spin_unlock(q: *mut c_void);
}

/// What the dequeue functions read, write and call: the queue's fields, its
/// lock, and the list operations they use.
///
/// Implemented here for the live C state. The harness implements it on loom's
/// atomics, so its models run [`dequeue()`] and [`dequeue_all()`] themselves.
/// Each method names the C operation it stands for.
pub(crate) trait Lwq {
    /// `lwq_empty(q)`: `smp_load_acquire(&q->ready) == NULL &&
    /// llist_empty(&q->new)`.
    fn empty(&self) -> bool;
    /// `spin_lock(&q->lock)`.
    fn lock(&self);
    /// `spin_unlock(&q->lock)`.
    fn unlock(&self);
    /// The plain reads of `q->ready`, which C does under the lock.
    fn ready(&self) -> *mut llist_node;
    /// The plain stores to `q->ready`, under the lock.
    fn set_ready(&self, node: *mut llist_node);
    /// `smp_store_release(&q->ready, node)`.
    fn release_ready(&self, node: *mut llist_node);
    /// `llist_empty(&q->new)`.
    fn new_is_empty(&self) -> bool;
    /// `llist_del_all(&q->new)`: `xchg()`, fully ordered.
    fn new_del_all(&self) -> *mut llist_node;
    /// `llist_next(node)`, and the reads of `node->next` in the walk.
    fn next(&self, node: *mut llist_node) -> *mut llist_node;
    /// The plain store `node->next = next`.
    fn set_next(&self, node: *mut llist_node, next: *mut llist_node);
    /// `llist_reverse_order(head)`.
    fn reverse_order(&self, head: *mut llist_node) -> *mut llist_node;
}

/// The queue that a call of an exported function runs on.
struct Live<'a> {
    /// The pointer the C wrappers take, kept whole for their provenance.
    q: *mut lwq,
    queue: &'a lwq,
}

impl Lwq for Live<'_> {
    #[inline]
    fn empty(&self) -> bool {
        // SAFETY: (U1) the C wrapper only does lwq_empty(q) on the caller's
        // live lwq.
        unsafe { c_lwq_empty(self.q.cast()) }
    }

    #[inline]
    fn lock(&self) {
        // SAFETY: (U1) the C wrapper only does spin_lock(&q->lock) on the
        // caller's live lwq; the matching unlock() follows in the same call.
        unsafe { c_lwq_spin_lock(self.q.cast()) }
    }

    #[inline]
    fn unlock(&self) {
        // SAFETY: (U1) this thread holds q->lock, taken by lock().
        unsafe { c_lwq_spin_unlock(self.q.cast()) }
    }

    #[inline]
    fn ready(&self) -> *mut llist_node {
        self.queue.ready.load(Ordering::Relaxed)
    }

    #[inline]
    fn set_ready(&self, node: *mut llist_node) {
        self.queue.ready.store(node, Ordering::Relaxed)
    }

    #[inline]
    fn release_ready(&self, node: *mut llist_node) {
        self.queue.ready.store(node, Ordering::Release)
    }

    #[inline]
    fn new_is_empty(&self) -> bool {
        self.queue.new.first().read_once().is_null()
    }

    #[inline]
    fn new_del_all(&self) -> *mut llist_node {
        llist::del_all(self.queue.new.first())
    }

    #[inline]
    fn next(&self, node: *mut llist_node) -> *mut llist_node {
        // SAFETY: (U3) `node` is an entry of the ready chain, which the caller
        // holds q->lock for, or of a chain it deleted from q->new and so
        // owns. It is live until its owner dequeues it.
        unsafe { &*node }.next().read_once()
    }

    #[inline]
    fn set_next(&self, node: *mut llist_node, next: *mut llist_node) {
        // SAFETY: (U3) as next(): the chain is owned by this call, and the
        // store is the plain `*ep = ...` of the C walk.
        unsafe { &*node }.next().write(next)
    }

    #[inline]
    fn reverse_order(&self, head: *mut llist_node) -> *mut llist_node {
        // SAFETY: (U1) `head` is NULL or the first entry of a chain that this
        // call deleted from q->new, which it owns, as llist_reverse_order()
        // requires.
        unsafe { llist::llist_reverse_order(head) }
    }
}

/// The body of `__lwq_dequeue()`, on any [`Lwq`] queue.
pub(crate) fn dequeue<Q: Lwq>(q: &Q) -> *mut llist_node {
    if q.empty() {
        return ptr::null_mut();
    }
    q.lock();
    let mut this = q.ready();
    if this.is_null() && !q.new_is_empty() {
        /* ensure queue doesn't appear transiently lwq_empty */
        q.release_ready(ptr::without_provenance_mut(1));
        this = q.reverse_order(q.new_del_all());
        if this.is_null() {
            q.set_ready(ptr::null_mut());
        }
    }
    if !this.is_null() {
        q.set_ready(q.next(this));
    }
    q.unlock();
    this
}

/// The body of `lwq_dequeue_all()`, on any [`Lwq`] queue.
pub(crate) fn dequeue_all<Q: Lwq>(q: &Q) -> *mut llist_node {
    if q.empty() {
        return ptr::null_mut();
    }
    q.lock();
    let r = q.ready();
    q.set_ready(ptr::null_mut());
    let t = q.new_del_all();
    q.unlock();

    // `ep = &r; while (*ep) ep = &(*ep)->next; *ep = llist_reverse_order(t);`:
    // the reversed new entries go after the last ready one.
    let t = q.reverse_order(t);
    if r.is_null() {
        return t;
    }
    let mut last = r;
    loop {
        let next = q.next(last);
        if next.is_null() {
            break;
        }
        last = next;
    }
    q.set_next(last, t);
    r
}

/// Dequeue the first (oldest) entry, or return NULL. This is the function
/// behind `lwq_dequeue()`.
///
/// # Safety
///
/// `q` is a live, initialized C `struct lwq`. The caller serializes dequeues
/// in one context, as `lwq_dequeue()` requires, and its entries stay live
/// until it dequeues them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __lwq_dequeue(q: *mut lwq) -> *mut llist_node {
    // SAFETY: (U3) the C contract of __lwq_dequeue(): q is the caller's live
    // lwq. Every field is interior-mutable, so a shared reference is sound
    // while C enqueues concurrently.
    let queue = unsafe { &*q };

    dequeue(&Live { q, queue })
}

/// lwq_dequeue_all - dequeue all currently enqueued objects
/// @q: the queue to dequeue from
///
/// Remove and return a linked list of llist_nodes of all the objects that were
/// in the queue. The first on the list will be the object that was least
/// recently enqueued.
///
/// # Safety
///
/// `q` is a live, initialized C `struct lwq`, and its entries stay live until
/// the caller dequeues them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lwq_dequeue_all(q: *mut lwq) -> *mut llist_node {
    // SAFETY: (U3) the C contract of lwq_dequeue_all(): q is the caller's live
    // lwq, and every field is interior-mutable.
    let queue = unsafe { &*q };

    dequeue_all(&Live { q, queue })
}
