// SPDX-License-Identifier: GPL-2.0-or-later

//! Generic Timer-queue
//!
//! Manages a simple queue of timers, ordered by expiration time.
//! Uses rbtrees for quick list adds and expiration.
//!
//! NOTE: All of the following functions need to be serialized
//! to avoid races. No locking is done by this library code.
//!
//! The Rust implementation of `lib/timerqueue.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/timerqueue.h` is the ABI contract,
//! and its static inlines stay in C. `lib/timerqueue_ffi.c` exports these
//! symbols with the license of `lib/timerqueue.c`.
//!
//! The rbtree helpers this unit used through `<linux/rbtree.h>` are static
//! inlines taking a comparison callback: `rb_add_cached()` and
//! `rb_add_linked()`. A function pointer across the FFI boundary would cost
//! an indirect call per comparison, so the descent loop is written here
//! instead, with the comparison inline, and only the out-of-line rebalancing
//! stays in C: `rb_insert_color()`, `rb_erase()` and `rb_next()` are exported
//! symbols. `rb_link_node()`, `rb_insert_color_cached()`, `rb_erase_cached()`,
//! `rb_first_cached()` and `rb_link_linked_node()` are pointer manipulation,
//! which the mirrors below make expressible here.

use core::ptr;

/// `ktime_t` (`include/linux/types.h`).
#[allow(non_camel_case_types)]
pub type ktime_t = i64;

/// Mirror of `struct rb_node` (`include/linux/rbtree_types.h`).
///
/// The C declaration is `__attribute__((aligned(sizeof(long))))`, which is
/// the alignment three pointers already have.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_node {
    __rb_parent_color: usize,
    rb_right: *mut rb_node,
    rb_left: *mut rb_node,
}

/// Mirror of `struct rb_root`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_root {
    rb_node: *mut rb_node,
}

/// Mirror of `struct rb_root_cached`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_root_cached {
    rb_root: rb_root,
    rb_leftmost: *mut rb_node,
}

/// Mirror of `struct rb_node_linked`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_node_linked {
    node: rb_node,
    prev: *mut rb_node_linked,
    next: *mut rb_node_linked,
}

/// Mirror of `struct rb_root_linked`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rb_root_linked {
    rb_root: rb_root,
    rb_leftmost: *mut rb_node_linked,
}

/// Mirror of `struct timerqueue_node` (`include/linux/timerqueue_types.h`).
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct timerqueue_node {
    node: rb_node,
    expires: ktime_t,
}

/// Mirror of `struct timerqueue_head`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct timerqueue_head {
    rb_root: rb_root_cached,
}

/// Mirror of `struct timerqueue_linked_node`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct timerqueue_linked_node {
    node: rb_node_linked,
    expires: ktime_t,
}

/// Mirror of `struct timerqueue_linked_head`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct timerqueue_linked_head {
    rb_root: rb_root_linked,
}

// The 64-bit values, from pahole of BTF of the running distro kernel and of
// DWARF of a tinyconfig SMP build. 32-bit is not asserted: no 32-bit build
// was measured, and guessing the padding of ktime_t after a 12-byte rb_node
// would be a claim without evidence.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(rb_node, size = 24, align = 8,
    __rb_parent_color @ 0, rb_right @ 8, rb_left @ 16);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(rb_root, size = 8, align = 8, rb_node @ 0);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(rb_root_cached, size = 16, align = 8,
    rb_root @ 0, rb_leftmost @ 8);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(rb_node_linked, size = 40, align = 8,
    node @ 0, prev @ 24, next @ 32);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(rb_root_linked, size = 16, align = 8,
    rb_root @ 0, rb_leftmost @ 8);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(timerqueue_node, size = 32, align = 8,
    node @ 0, expires @ 24);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(timerqueue_head, size = 16, align = 8, rb_root @ 0);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(timerqueue_linked_node, size = 48, align = 8,
    node @ 0, expires @ 40);
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(timerqueue_linked_head, size = 16, align = 8, rb_root @ 0);

unsafe extern "C" {
    fn rb_insert_color(node: *mut rb_node, root: *mut rb_root);
    fn rb_erase(node: *mut rb_node, root: *mut rb_root);
    fn rb_next(node: *const rb_node) -> *mut rb_node;

    // One wrapper per WARN_ON_ONCE() site in lib/timerqueue.c, so that each
    // keeps its own once flag.
    fn c_timerqueue_warn_add_queued();
    fn c_timerqueue_warn_del_not_queued();
}

/// `RB_EMPTY_NODE()`: a node known not to be in a tree points at itself.
///
/// # Safety
///
/// `node` points to a live `rb_node`.
unsafe fn rb_empty_node(node: *const rb_node) -> bool {
    // SAFETY: (U3) the caller passes a live rb_node, which the timerqueue
    // contract says it serializes against every other user.
    unsafe { (*node).__rb_parent_color == node as usize }
}

/// `RB_CLEAR_NODE()`.
///
/// # Safety
///
/// `node` points to a live `rb_node`.
unsafe fn rb_clear_node(node: *mut rb_node) {
    // SAFETY: (U3) as rb_empty_node().
    unsafe { (*node).__rb_parent_color = node as usize };
}

/// `rb_link_node()`.
///
/// # Safety
///
/// `node` points to a live `rb_node`, and `link` to the place in the tree
/// that the descent below settled on.
unsafe fn rb_link_node(node: *mut rb_node, parent: *mut rb_node, link: *mut *mut rb_node) {
    // SAFETY: (U3) the descent produced link, and the caller serializes.
    unsafe {
        (*node).__rb_parent_color = parent as usize;
        (*node).rb_left = ptr::null_mut();
        (*node).rb_right = ptr::null_mut();
        *link = node;
    }
}

/// `__node_2_tq()`: `rb_entry()` with `node` at offset 0.
fn node_2_tq(node: *mut rb_node) -> *mut timerqueue_node {
    node.cast()
}

/// `__node_2_tq_linked()`: `node` is at offset 0 of both structures.
fn node_2_tq_linked(node: *mut rb_node) -> *mut timerqueue_linked_node {
    node.cast()
}

/// `timerqueue_add()`: adds timer to timerqueue.
///
/// Adds the timer node to the timerqueue, sorted by the node's expires
/// value. Returns true if the newly added timer is the first expiring timer
/// in the queue.
///
/// # Safety
///
/// `head` and `node` point to live objects, and the caller serializes this
/// call against every other user of `head`, as the unit requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn timerqueue_add(
    head: *mut timerqueue_head,
    node: *mut timerqueue_node,
) -> bool {
    // SAFETY: (U3) the caller passes live objects and serializes.
    let (head, node) = unsafe { (&mut *head, &mut *node) };

    /* Make sure we don't add nodes that are already added */
    // SAFETY: (U3) node.node is the rb_node of a live timerqueue_node.
    if !unsafe { rb_empty_node(&node.node) } {
        // SAFETY: (U1) c_timerqueue_warn_add_queued() takes no arguments and
        // only WARN_ON_ONCE()s.
        unsafe { c_timerqueue_warn_add_queued() };
    }

    // rb_add_cached() with __timerqueue_less(): descend to the leaf, keeping
    // track of whether the new node is the leftmost.
    let expires = node.expires;
    let tree = &mut head.rb_root;
    let mut link: *mut *mut rb_node = &mut tree.rb_root.rb_node;
    let mut parent: *mut rb_node = ptr::null_mut();
    let mut leftmost = true;

    // SAFETY: (U3) every pointer walked here comes from the tree the caller
    // owns for the duration of this call.
    unsafe {
        while !(*link).is_null() {
            parent = *link;
            if expires < (*node_2_tq(parent)).expires {
                link = &mut (*parent).rb_left;
            } else {
                link = &mut (*parent).rb_right;
                leftmost = false;
            }
        }

        rb_link_node(&mut node.node, parent, link);

        // rb_insert_color_cached().
        if leftmost {
            tree.rb_leftmost = &mut node.node;
        }
        rb_insert_color(&mut node.node, &mut tree.rb_root);
    }

    leftmost
}

/// `timerqueue_del()`: removes a timer from the timerqueue.
///
/// Removes the timer node from the timerqueue. Returns true if the queue is
/// not empty after the remove.
///
/// # Safety
///
/// As [`timerqueue_add()`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn timerqueue_del(
    head: *mut timerqueue_head,
    node: *mut timerqueue_node,
) -> bool {
    // SAFETY: (U3) the caller passes live objects and serializes.
    let (head, node) = unsafe { (&mut *head, &mut *node) };

    // SAFETY: (U3) node.node is the rb_node of a live timerqueue_node.
    if unsafe { rb_empty_node(&node.node) } {
        // SAFETY: (U1) c_timerqueue_warn_del_not_queued() takes no arguments
        // and only WARN_ON_ONCE()s.
        unsafe { c_timerqueue_warn_del_not_queued() };
    }

    let tree = &mut head.rb_root;

    // SAFETY: (U3) the node is in the tree the caller owns for this call.
    unsafe {
        // rb_erase_cached(): the leftmost moves on before the erase.
        if ptr::eq(tree.rb_leftmost, &node.node) {
            tree.rb_leftmost = rb_next(&node.node);
        }
        rb_erase(&mut node.node, &mut tree.rb_root);
        rb_clear_node(&mut node.node);
    }

    // RB_EMPTY_ROOT()
    !tree.rb_root.rb_node.is_null()
}

/// `timerqueue_iterate_next()`: returns the timer after the provided timer.
///
/// Provides the timer that is after the given node. This is used, when
/// necessary, to iterate through the list of timers in a timer list
/// without modifying the list.
///
/// # Safety
///
/// `node` is null or points to a live `timerqueue_node` in a tree the caller
/// serializes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn timerqueue_iterate_next(
    node: *mut timerqueue_node,
) -> *mut timerqueue_node {
    if node.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: (U3) node is live, and rb_next() walks the tree the caller
    // serializes.
    let next = unsafe { rb_next(&(*node).node) };
    if next.is_null() {
        return ptr::null_mut();
    }

    node_2_tq(next)
}

/// `timerqueue_linked_add()`: adds a node to a timerqueue with linked nodes.
///
/// Returns true when the node is the new leftmost.
///
/// # Safety
///
/// As [`timerqueue_add()`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn timerqueue_linked_add(
    head: *mut timerqueue_linked_head,
    node: *mut timerqueue_linked_node,
) -> bool {
    // SAFETY: (U3) the caller passes live objects and serializes.
    let (head, node) = unsafe { (&mut *head, &mut *node) };

    // rb_add_linked() over __rb_add() with __tq_linked_less().
    let expires = node.expires;
    let tree = &mut head.rb_root;
    let mut link: *mut *mut rb_node = &mut tree.rb_root.rb_node;
    let mut parent: *mut rb_node = ptr::null_mut();

    // SAFETY: (U3) every pointer walked here comes from the tree the caller
    // owns for the duration of this call.
    unsafe {
        while !(*link).is_null() {
            parent = *link;
            if expires < (*node_2_tq_linked(parent)).expires {
                link = &mut (*parent).rb_left;
            } else {
                link = &mut (*parent).rb_right;
            }
        }

        // rb_link_linked_node(): splice the new node into the ordered list of
        // its neighbours, which is what makes next and prev O(1).
        if !parent.is_null() {
            let nnew = &mut node.node;
            let npar = parent.cast::<rb_node_linked>();

            if ptr::addr_eq(link, ptr::addr_of!((*parent).rb_left)) {
                nnew.prev = (*npar).prev;
                nnew.next = npar;
                (*npar).prev = nnew;
                if !nnew.prev.is_null() {
                    (*nnew.prev).next = nnew;
                }
            } else {
                nnew.next = (*npar).next;
                nnew.prev = npar;
                (*npar).next = nnew;
                if !nnew.next.is_null() {
                    (*nnew.next).prev = nnew;
                }
            }
        }

        rb_link_node(&mut node.node.node, parent, link);
        rb_insert_color(&mut node.node.node, &mut tree.rb_root);
    }

    if node.node.prev.is_null() {
        tree.rb_leftmost = &mut node.node;
    }

    node.node.prev.is_null()
}
