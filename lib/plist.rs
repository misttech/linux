// SPDX-License-Identifier: GPL-2.0-or-later

//! lib/plist.rs
//!
//! Descending-priority-sorted double-linked list, replacing `lib/plist.c` in place.
//!
//! (C) 2002-2003 Intel Corp
//! Inaky Perez-Gonzalez <inaky.perez-gonzalez@intel.com>.
//!
//! 2001-2005 (c) MontaVista Software, Inc.
//! Daniel Walker <dwalker@mvista.com>
//!
//! (C) 2005 Linutronix GmbH, Thomas Gleixner <tglx@kernel.org>
//!
//! Simplifications of the original code by
//! Oleg Nesterov <oleg@tv-sign.ru>
//!
//! Based on simple lists (include/linux/list.h).
//!
//! This file contains the add / del functions which are considered to
//! be too large to inline. See include/linux/plist.h for further
//! information.
//!
//! A `plist_node` is on two lists at once: every node is on the `node_list` of its
//! head, sorted by priority, and the first node of each priority is also on the
//! `prio_list`, which links one node per priority. The port keeps the C's use of
//! `<linux/list.h>`: the three list operations that change a list, `list_add()`,
//! `list_add_tail()` and `list_del_init()`, stay in C, reached through
//! `c_plist_list_*` wrappers, so a hardened kernel keeps its list corruption
//! checks. The reads (`list_empty()`, `->next`, `->prev`) are `Relaxed` loads, which
//! is what a plain C read of a list a lock protects compiles to.
//!
//! No caller of `plist_add()`, `plist_del()` or `plist_requeue()` changes: the
//! functions have the same names and signatures, and `plist_node_init()` and the
//! other inlines of `<linux/plist.h>` are still C.

use core::ffi::c_int;
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

unsafe extern "C" {
    /// `list_add()`, a static inline.
    fn c_plist_list_add(new: *mut list_head, head: *mut list_head);
    /// `list_add_tail()`, a static inline.
    fn c_plist_list_add_tail(new: *mut list_head, head: *mut list_head);
    /// `list_del_init()`, a static inline.
    fn c_plist_list_del_init(entry: *mut list_head);
    /// `WARN_ON(!plist_node_empty(node))` of `plist_add()`, a macro.
    fn c_plist_warn_add_node_linked();
    /// `WARN_ON(!list_empty(&node->prio_list))` of `plist_add()`, a macro.
    fn c_plist_warn_add_prio_list_linked();
    /// `BUG_ON()`, a macro: a Rust panic is not `BUG()`, and an extern function that never
    /// returns is one that objtool cannot know about.
    fn c_plist_bug_on(condition: bool);
}

/// `struct list_head`. The fields are atomics so that a plain access from C does not
/// make a shared reference to them undefined; they are read `Relaxed`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct list_head {
    next: AtomicPtr<list_head>,
    prev: AtomicPtr<list_head>,
}

/// `struct plist_head`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct plist_head {
    node_list: list_head,
}

/// `struct plist_node`. `prio` is written by C callers while the node is on no list, so
/// it is an atomic too.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct plist_node {
    prio: AtomicI32,
    prio_list: list_head,
    node_list: list_head,
}

// BTF of x86_64 kernels: plist_head is one list_head, and plist_node has a hole of 4
// bytes after prio. 32-bit values are from DWARF of clang --target=i386 for the same
// structs (include/linux/plist_types.h): sizes 8, 8 and 20, align 4. None depends on the
// configuration.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(list_head, size = 16, align = 8, next @ 0, prev @ 8);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(plist_head, size = 16, align = 8, node_list @ 0);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(
    plist_node,
    size = 40,
    align = 8,
    prio @ 0,
    prio_list @ 8,
    node_list @ 24
);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(list_head, size = 8, align = 4, next @ 0, prev @ 4);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(plist_head, size = 8, align = 4, node_list @ 0);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(
    plist_node,
    size = 20,
    align = 4,
    prio @ 0,
    prio_list @ 4,
    node_list @ 12
);

/// `&node->prio_list`.
fn prio_list(node: *mut plist_node) -> *mut list_head {
    node.wrapping_byte_add(mem::offset_of!(plist_node, prio_list))
        .cast()
}

/// `&node->node_list`.
fn node_list(node: *mut plist_node) -> *mut list_head {
    node.wrapping_byte_add(mem::offset_of!(plist_node, node_list))
        .cast()
}

/// `&head->node_list`.
fn head_list(head: *mut plist_head) -> *mut list_head {
    head.wrapping_byte_add(mem::offset_of!(plist_head, node_list))
        .cast()
}

/// `list_entry(l, struct plist_node, prio_list)`. Nothing is read: `l` need not be
/// on a list, and the result is only the address of the node around it.
fn from_prio_list(l: *mut list_head) -> *mut plist_node {
    l.wrapping_byte_sub(mem::offset_of!(plist_node, prio_list))
        .cast()
}

/// `list_entry(l, struct plist_node, node_list)`.
fn from_node_list(l: *mut list_head) -> *mut plist_node {
    l.wrapping_byte_sub(mem::offset_of!(plist_node, node_list))
        .cast()
}

/// `l->next`.
///
/// # Safety
///
/// `l` points to a live `list_head` that the caller's lock protects.
unsafe fn list_next(l: *mut list_head) -> *mut list_head {
    // SAFETY: (U3) the caller's contract: `l` is a live list_head.
    unsafe { &*l }.next.load(Ordering::Relaxed)
}

/// `l->prev`.
///
/// # Safety
///
/// As for [`list_next()`].
unsafe fn list_prev(l: *mut list_head) -> *mut list_head {
    // SAFETY: (U3) the caller's contract: `l` is a live list_head.
    unsafe { &*l }.prev.load(Ordering::Relaxed)
}

/// `node->prio`.
///
/// # Safety
///
/// `node` points to a live `plist_node`.
unsafe fn prio(node: *mut plist_node) -> c_int {
    // SAFETY: (U3) the caller's contract: `node` is a live plist_node.
    unsafe { &*node }.prio.load(Ordering::Relaxed)
}

/// `list_empty()`.
///
/// # Safety
///
/// As for [`list_next()`].
unsafe fn list_empty(l: *mut list_head) -> bool {
    // SAFETY: (U3) the caller's contract: `l` is a live list_head.
    unsafe { list_next(l) == l }
}

/// `plist_head_empty()`.
///
/// # Safety
///
/// `head` points to a live `plist_head`.
unsafe fn plist_head_empty(head: *mut plist_head) -> bool {
    // SAFETY: (U3) the caller's contract: the head is live.
    unsafe { list_empty(head_list(head)) }
}

/// `plist_node_empty()`.
///
/// # Safety
///
/// `node` points to a live `plist_node`.
unsafe fn plist_node_empty(node: *mut plist_node) -> bool {
    // SAFETY: (U3) the caller's contract: the node is live.
    unsafe { list_empty(node_list(node)) }
}

/// `plist_first()`.
///
/// # Safety
///
/// As for [`plist_head_empty()`].
unsafe fn plist_first(head: *mut plist_head) -> *mut plist_node {
    // SAFETY: (U3) the caller's contract: the head is live.
    from_node_list(unsafe { list_next(head_list(head)) })
}

/// `plist_last()`.
///
/// # Safety
///
/// As for [`plist_head_empty()`].
unsafe fn plist_last(head: *mut plist_head) -> *mut plist_node {
    // SAFETY: (U3) the caller's contract: the head is live.
    from_node_list(unsafe { list_prev(head_list(head)) })
}

/// `list_add()`.
///
/// # Safety
///
/// `new` is a live `list_head` that is on no list, and `head` a live one, under the
/// lock that protects the list.
unsafe fn list_add(new: *mut list_head, head: *mut list_head) {
    // SAFETY: (U1) list_add(): the caller's contract.
    unsafe { c_plist_list_add(new, head) }
}

/// `list_add_tail()`.
///
/// # Safety
///
/// As for [`list_add()`].
unsafe fn list_add_tail(new: *mut list_head, head: *mut list_head) {
    // SAFETY: (U1) list_add_tail(): the caller's contract.
    unsafe { c_plist_list_add_tail(new, head) }
}

/// `list_del_init()`.
///
/// # Safety
///
/// `entry` is a live `list_head` that is on a list, or empty, under the lock that
/// protects the list.
unsafe fn list_del_init(entry: *mut list_head) {
    // SAFETY: (U1) list_del_init(): the caller's contract.
    unsafe { c_plist_list_del_init(entry) }
}

/// `WARN_ON(!plist_node_empty(node))` of `plist_add()`.
fn warn_add_node_linked() {
    // SAFETY: (U1) WARN_ON(): takes no argument.
    unsafe { c_plist_warn_add_node_linked() }
}

/// `WARN_ON(!list_empty(&node->prio_list))` of `plist_add()`.
fn warn_add_prio_list_linked() {
    // SAFETY: (U1) WARN_ON(): takes no argument.
    unsafe { c_plist_warn_add_prio_list_linked() }
}

/// `BUG_ON(condition)`.
fn bug_on(condition: bool) {
    // SAFETY: (U1) BUG_ON(): takes a bool.
    unsafe { c_plist_bug_on(condition) }
}

#[cfg(CONFIG_DEBUG_PLIST)]
mod debug {
    use super::*;

    unsafe extern "C" {
        /// `WARN(...)` of `plist_check_prev_next()`, a macro, which prints the nine
        /// pointers around the break: each of `t`, `p` and `n`, and its `next` and `prev`.
        #[allow(clippy::too_many_arguments)]
        fn c_plist_warn_prev_next(
            t: *mut list_head,
            t_next: *mut list_head,
            t_prev: *mut list_head,
            p: *mut list_head,
            p_next: *mut list_head,
            p_prev: *mut list_head,
            n: *mut list_head,
            n_next: *mut list_head,
            n_prev: *mut list_head,
        );
    }

    /// # Safety
    ///
    /// `t`, `p` and `n` are live `list_head`s under the lock that protects the list.
    unsafe fn plist_check_prev_next(t: *mut list_head, p: *mut list_head, n: *mut list_head) {
        // SAFETY: (U3) the caller's contract: all three are live list_heads.
        if unsafe { list_prev(n) != p || list_next(p) != n } {
            // SAFETY: (U3) as above.
            let (t_next, t_prev, p_next, p_prev, n_next, n_prev) = unsafe {
                (
                    list_next(t),
                    list_prev(t),
                    list_next(p),
                    list_prev(p),
                    list_next(n),
                    list_prev(n),
                )
            };

            // SAFETY: (U1) the macro only prints the pointers.
            unsafe {
                c_plist_warn_prev_next(t, t_next, t_prev, p, p_next, p_prev, n, n_next, n_prev)
            }
        }
    }

    /// # Safety
    ///
    /// `top` is a live `list_head` of a well-formed circular list, under its lock.
    unsafe fn plist_check_list(top: *mut list_head) {
        // SAFETY: (U3) the caller's contract: the list is live, and every node on it.
        unsafe {
            let mut prev = top;
            let mut next = list_next(top);

            plist_check_prev_next(top, prev, next);
            while next != top {
                prev = next;
                next = list_next(prev);
                plist_check_prev_next(top, prev, next);
            }
        }
    }

    /// # Safety
    ///
    /// `head` is a live `plist_head`, under its lock.
    pub(super) unsafe fn plist_check_head(head: *mut plist_head) {
        // SAFETY: (U3) the caller's contract: the head and its nodes are live.
        unsafe {
            if !plist_head_empty(head) {
                plist_check_list(prio_list(plist_first(head)));
            }
            plist_check_list(head_list(head));
        }
    }
}

/// With `CONFIG_DEBUG_PLIST` the whole list is walked and its links checked.
///
/// # Safety
///
/// `head` is a live `plist_head`, under its lock.
#[cfg(CONFIG_DEBUG_PLIST)]
unsafe fn plist_check_head(head: *mut plist_head) {
    // SAFETY: (U3) the caller's contract: the head and its nodes are live.
    unsafe { debug::plist_check_head(head) }
}

/// # Safety
///
/// `_head` is a live `plist_head`. Nothing reads it.
#[cfg(not(CONFIG_DEBUG_PLIST))]
unsafe fn plist_check_head(_head: *mut plist_head) {}

/// plist_add - add @node to @head
///
/// @node:    &struct plist_node pointer
/// @head:    &struct plist_head pointer
///
/// # Safety
///
/// `node` is a live `plist_node`, and `head` a live `plist_head`, whose nodes are all
/// live. The caller holds the lock that protects the list, and nothing else changes
/// `node` during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plist_add(node: *mut plist_node, head: *mut plist_head) {
    // SAFETY: (U3) the caller's contract: the head, every node on it and `node` are
    // live and protected by the caller's lock, and so is every list_head inside them.
    unsafe {
        let mut prev: *mut plist_node = ptr::null_mut();
        let mut node_next: *mut list_head = head_list(head);

        plist_check_head(head);
        if !plist_node_empty(node) {
            warn_add_node_linked();
        }
        if !list_empty(prio_list(node)) {
            warn_add_prio_list_linked();
        }

        if !plist_head_empty(head) {
            let first = plist_first(head);
            let mut iter = first;
            let last = from_prio_list(list_prev(prio_list(first)));
            let mut reverse_iter = last;

            loop {
                if prio(node) < prio(iter) {
                    node_next = node_list(iter);
                    break;
                } else if prio(node) >= prio(reverse_iter) {
                    prev = reverse_iter;
                    iter = from_prio_list(list_next(prio_list(reverse_iter)));
                    if reverse_iter != last {
                        node_next = node_list(iter);
                    }
                    break;
                }

                prev = iter;
                iter = from_prio_list(list_next(prio_list(iter)));
                reverse_iter = from_prio_list(list_prev(prio_list(reverse_iter)));
                if iter == first {
                    break;
                }
            }

            if prev.is_null() || prio(prev) != prio(node) {
                list_add_tail(prio_list(node), prio_list(iter));
            }
        }
        list_add_tail(node_list(node), node_next);

        plist_check_head(head);
    }
}

/// plist_del - Remove a @node from plist.
///
/// @node:    &struct plist_node pointer - entry to be removed
/// @head:    &struct plist_head pointer - list head
///
/// # Safety
///
/// As for [`plist_add()`], except that `node` is on `head`, or on no list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plist_del(node: *mut plist_node, head: *mut plist_head) {
    // SAFETY: (U3) the caller's contract, as plist_add().
    unsafe {
        plist_check_head(head);

        if !list_empty(prio_list(node)) {
            if list_next(node_list(node)) != head_list(head) {
                let next = from_node_list(list_next(node_list(node)));

                /* add the next plist_node into prio_list */
                if list_empty(prio_list(next)) {
                    list_add(prio_list(next), prio_list(node));
                }
            }
            list_del_init(prio_list(node));
        }

        list_del_init(node_list(node));

        plist_check_head(head);
    }
}

/// plist_requeue - Requeue @node at end of same-prio entries.
///
/// This is essentially an optimized plist_del() followed by
/// plist_add().  It moves an entry already in the plist to
/// after any other same-priority entries.
///
/// @node:    &struct plist_node pointer - entry to be moved
/// @head:    &struct plist_head pointer - list head
///
/// # Safety
///
/// As for [`plist_add()`], except that `node` is on `head`, and `head` is not empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plist_requeue(node: *mut plist_node, head: *mut plist_head) {
    // SAFETY: (U3) the caller's contract, as plist_add().
    unsafe {
        let mut node_next: *mut list_head = head_list(head);

        plist_check_head(head);
        bug_on(plist_head_empty(head));
        bug_on(plist_node_empty(node));

        if node == plist_last(head) {
            return;
        }

        let mut iter = from_node_list(list_next(node_list(node)));

        if prio(node) != prio(iter) {
            return;
        }

        plist_del(node, head);

        /*
         * After plist_del(), iter is the replacement of the node.  If the node
         * was on prio_list, take shortcut to find node_next instead of looping.
         */
        if !list_empty(prio_list(iter)) {
            iter = from_prio_list(list_next(prio_list(iter)));
            node_next = node_list(iter);
        } else {
            /* plist_for_each_continue(iter, head) */
            iter = from_node_list(list_next(node_list(iter)));
            while node_list(iter) != head_list(head) {
                if prio(node) != prio(iter) {
                    node_next = node_list(iter);
                    break;
                }
                iter = from_node_list(list_next(node_list(iter)));
            }
        }

        list_add_tail(node_list(node), node_next);

        plist_check_head(head);
    }
}

/// The boot-time test of `lib/plist.c`, run by `module_init()` in `lib/plist_ffi.c`:
/// with `CONFIG_DEBUG_PLIST` a kernel that runs it exercises the exports with the
/// C's own checks, which `BUG()` on the first wrong link.
#[cfg(CONFIG_DEBUG_PLIST)]
mod test {
    use super::*;

    unsafe extern "C" {
        /// `local_clock()`, truncated to an `unsigned int` as the C test does.
        fn c_plist_local_clock() -> u32;
        /// `ktime_get()`.
        fn c_plist_ktime_get() -> i64;
        /// `printk(KERN_DEBUG "start plist test\n")`.
        fn c_plist_debug_start();
        /// `printk(KERN_DEBUG "end plist test\n")`.
        fn c_plist_debug_end();
        /// `pr_debug("plist_add worst case test time elapsed %lld\n", elapsed)`.
        fn c_plist_debug_elapsed(elapsed: i64);
    }

    const NODES: usize = 241;

    static TEST_HEAD: plist_head = plist_head::new();
    static TEST_NODE: [plist_node; NODES] = [const { plist_node::new() }; NODES];

    impl list_head {
        const fn new() -> Self {
            Self {
                next: AtomicPtr::new(ptr::null_mut()),
                prev: AtomicPtr::new(ptr::null_mut()),
            }
        }
    }

    impl plist_head {
        const fn new() -> Self {
            Self {
                node_list: list_head::new(),
            }
        }
    }

    impl plist_node {
        const fn new() -> Self {
            Self {
                prio: AtomicI32::new(0),
                prio_list: list_head::new(),
                node_list: list_head::new(),
            }
        }
    }

    /// `INIT_LIST_HEAD()`.
    fn init_list_head(l: *mut list_head) {
        // SAFETY: (U3) `l` is a list_head inside the test statics.
        let l_ref = unsafe { &*l };

        l_ref.next.store(l, Ordering::Relaxed);
        l_ref.prev.store(l, Ordering::Relaxed);
    }

    /// `plist_head_init()`.
    fn plist_head_init(head: *mut plist_head) {
        init_list_head(head_list(head));
    }

    /// `plist_node_init()`.
    fn plist_node_init(node: *mut plist_node, prio: c_int) {
        // SAFETY: (U3) `node` is a plist_node of the test statics.
        unsafe { &*node }.prio.store(prio, Ordering::Relaxed);
        init_list_head(prio_list(node));
        init_list_head(node_list(node));
    }

    fn head() -> *mut plist_head {
        ptr::from_ref(&TEST_HEAD).cast_mut()
    }

    fn node(i: usize) -> *mut plist_node {
        ptr::from_ref(&TEST_NODE[i]).cast_mut()
    }

    /// `plist_test_check()`.
    fn plist_test_check(mut nr_expect: c_int) {
        // SAFETY: (U3) the test is the only user of the statics, and every node on the
        // test head is one of them.
        unsafe {
            if plist_head_empty(head()) {
                bug_on(nr_expect != 0);
                return;
            }

            let first = plist_first(head());
            let mut prio_pos = first;
            let mut node_pos = first;

            /* plist_for_each(node_pos, &test_head) */
            while node_list(node_pos) != head_list(head()) {
                let expect = nr_expect;
                nr_expect -= 1;
                if expect < 0 {
                    break;
                }
                if node_pos != first {
                    if prio(node_pos) == prio(prio_pos) {
                        bug_on(!list_empty(prio_list(node_pos)));
                    } else {
                        bug_on(prio(prio_pos) > prio(node_pos));
                        bug_on(list_next(prio_list(prio_pos)) != prio_list(node_pos));
                        prio_pos = node_pos;
                    }
                }
                node_pos = from_node_list(list_next(node_list(node_pos)));
            }

            bug_on(nr_expect != 0);
            bug_on(list_next(prio_list(prio_pos)) != prio_list(first));
        }
    }

    /// `plist_test_requeue()`.
    fn plist_test_requeue(node: *mut plist_node) {
        // SAFETY: (U3) as plist_test_check(): `node` is a test node, on the test head.
        unsafe {
            plist_requeue(node, head());

            if node != plist_last(head()) {
                bug_on(prio(node) == prio(from_node_list(list_next(node_list(node)))));
            }
        }
    }

    /// `plist_test()`, without the `module_init()`.
    #[unsafe(no_mangle)]
    pub(super) extern "C" fn plist_test_run() -> c_int {
        let mut nr_expect: c_int = 0;
        // SAFETY: (U1) local_clock(): no argument, no side effect.
        let mut r: u32 = unsafe { c_plist_local_clock() };

        // SAFETY: (U1) printk(): a constant string.
        unsafe { c_plist_debug_start() };
        plist_head_init(head());
        for i in 0..NODES {
            plist_node_init(node(i), 0);
        }

        for _ in 0..1000 {
            r = r.wrapping_mul(193939) % 47629;
            let i = r as usize % NODES;
            // SAFETY: (U3) the test is the only user of the statics.
            if unsafe { plist_node_empty(node(i)) } {
                r = r.wrapping_mul(193939) % 47629;
                // SAFETY: (U3) as above.
                unsafe { &*node(i) }
                    .prio
                    .store((r % 99) as c_int, Ordering::Relaxed);
                // SAFETY: (U3) as above; the test holds no lock and nothing else runs.
                unsafe { plist_add(node(i), head()) };
                nr_expect += 1;
            } else {
                // SAFETY: (U3) as above.
                unsafe { plist_del(node(i), head()) };
                nr_expect -= 1;
            }
            plist_test_check(nr_expect);
            // SAFETY: (U3) as above.
            if !unsafe { plist_node_empty(node(i)) } {
                plist_test_requeue(node(i));
                plist_test_check(nr_expect);
            }
        }

        for i in 0..NODES {
            // SAFETY: (U3) as above.
            if unsafe { plist_node_empty(node(i)) } {
                continue;
            }
            // SAFETY: (U3) as above.
            unsafe { plist_del(node(i), head()) };
            nr_expect -= 1;
            plist_test_check(nr_expect);
        }

        // SAFETY: (U1) printk(): a constant string.
        unsafe { c_plist_debug_end() };

        /* Worst case test for plist_add() */
        let mut time_elapsed: i64 = 0;

        plist_head_init(head());

        for i in 0..NODES {
            plist_node_init(node(i), 0);
            // SAFETY: (U3) as above.
            unsafe { &*node(i) }
                .prio
                .store(i as c_int, Ordering::Relaxed);
        }

        for i in 0..NODES {
            // SAFETY: (U3) as above.
            if unsafe { plist_node_empty(node(i)) } {
                // SAFETY: (U1) ktime_get(): no argument.
                let start = unsafe { c_plist_ktime_get() };
                // SAFETY: (U3) as above.
                unsafe { plist_add(node(i), head()) };
                // SAFETY: (U1) as above.
                let end = unsafe { c_plist_ktime_get() };
                time_elapsed += end - start;
            }
        }

        // SAFETY: (U1) pr_debug(): a constant format and a number.
        unsafe { c_plist_debug_elapsed(time_elapsed) };
        0
    }
}
