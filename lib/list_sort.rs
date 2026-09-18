// SPDX-License-Identifier: GPL-2.0

//! Sort a doubly-linked list with a caller-supplied comparison.
//!
//! The Rust implementation of `lib/list_sort.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/list_sort.h` is the ABI contract, and
//! `lib/list_sort_ffi.c` exports `list_sort` with the license of
//! `lib/list_sort.c`.
//!
//! The comparison stays a C function pointer: it belongs to the caller, so it
//! cannot be inlined here the way `lib/timerqueue.rs` inlines its own
//! comparison, and every call through it is class U1.

use core::ptr;

/// Mirror of `struct list_head`.
///
/// The list is manipulated through raw pointers throughout: the nodes belong
/// to the caller, who the contract of `list_sort()` says has serialized this
/// call against every other user of the list.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct list_head {
    next: *mut list_head,
    prev: *mut list_head,
}

// Two pointers, from `pahole -C list_head` over the BTF of the running kernel
// and over the DWARF of a tinyconfig SMP build. 32-bit is the same shape with
// 4-byte pointers; no 32-bit build was measured, so it is asserted from the
// pointer width rather than from a measurement of `include/linux/types.h`.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(list_head, size = 16, align = 8, next @ 0, prev @ 8);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(list_head, size = 8, align = 4, next @ 0, prev @ 4);

/// `list_cmp_func_t`: the caller's comparison.
///
/// `include/linux/list_sort.h` marks the two node arguments `nonnull(2,3)`,
/// and the C code never calls it with a null node.
#[allow(non_camel_case_types)]
pub type list_cmp_func_t = unsafe extern "C" fn(
    priv_: *mut core::ffi::c_void,
    a: *const list_head,
    b: *const list_head,
) -> core::ffi::c_int;

/// Returns a list organized in an intermediate format suited
/// to chaining of merge() calls: null-terminated, no reserved or
/// sentinel head node, "prev" links not maintained.
///
/// # Safety
///
/// (U3) `a` and `b` are non-null heads of null-terminated singly-linked
/// lists, and `cmp` is the caller's comparison, as `list_sort()` guarantees.
unsafe fn merge(
    priv_: *mut core::ffi::c_void,
    cmp: list_cmp_func_t,
    mut a: *mut list_head,
    mut b: *mut list_head,
) -> *mut list_head {
    let mut head: *mut list_head = ptr::null_mut();
    // `tail` is C's `struct list_head **tail`: where the next node is stored,
    // either the local `head` or some node's `next` field.
    let mut tail: *mut *mut list_head = &mut head;

    loop {
        // if equal, take 'a' -- important for sort stability
        // SAFETY: (U1) the caller's comparison, called with two non-null
        // nodes, which is what `nonnull(2,3)` in the header requires.
        if unsafe { cmp(priv_, a, b) } <= 0 {
            // SAFETY: (U3) `tail` points at `head` or at a node's `next`, and
            // `a` is a live node of the caller's list.
            unsafe {
                *tail = a;
                tail = &mut (*a).next;
                a = (*a).next;
            }
            if a.is_null() {
                // SAFETY: (U3) as above; `b` still has nodes.
                unsafe { *tail = b };
                break;
            }
        } else {
            // SAFETY: (U3) as above, for `b`.
            unsafe {
                *tail = b;
                tail = &mut (*b).next;
                b = (*b).next;
            }
            if b.is_null() {
                // SAFETY: (U3) as above; `a` still has nodes.
                unsafe { *tail = a };
                break;
            }
        }
    }
    head
}

/// Combine final list merge with restoration of standard doubly-linked
/// list structure.  This approach duplicates code from merge(), but
/// runs faster than the tidier alternatives of either a separate final
/// prev-link restoration pass, or maintaining the prev links
/// throughout.
///
/// # Safety
///
/// (U3) `head` is the caller's list head, and `a` and `b` are non-null heads
/// of null-terminated singly-linked lists.
unsafe fn merge_final(
    priv_: *mut core::ffi::c_void,
    cmp: list_cmp_func_t,
    head: *mut list_head,
    mut a: *mut list_head,
    mut b: *mut list_head,
) {
    let mut tail = head;

    loop {
        // if equal, take 'a' -- important for sort stability
        // SAFETY: (U1) the caller's comparison, with two non-null nodes.
        if unsafe { cmp(priv_, a, b) } <= 0 {
            // SAFETY: (U3) `tail` and `a` are live nodes of the caller's list.
            unsafe {
                (*tail).next = a;
                (*a).prev = tail;
                tail = a;
                a = (*a).next;
            }
            if a.is_null() {
                break;
            }
        } else {
            // SAFETY: (U3) as above, for `b`.
            unsafe {
                (*tail).next = b;
                (*b).prev = tail;
                tail = b;
                b = (*b).next;
            }
            if b.is_null() {
                b = a;
                break;
            }
        }
    }

    // Finish linking remainder of list b on to tail
    // SAFETY: (U3) `tail` is a live node and `b` heads the remaining list.
    unsafe { (*tail).next = b };
    loop {
        // SAFETY: (U3) `b` is a live node of the caller's list.
        unsafe {
            (*b).prev = tail;
            tail = b;
            b = (*b).next;
        }
        if b.is_null() {
            break;
        }
    }

    // And the final links to make a circular doubly-linked list
    // SAFETY: (U3) `tail` and `head` are live.
    unsafe {
        (*tail).next = head;
        (*head).prev = tail;
    }
}

/// list_sort - sort a list
///
/// See `include/linux/list_sort.h` and the comment block in
/// `lib/list_sort.c`: the comparison must be antisymmetric and transitive,
/// the sort is stable, and it performs at least 2:1 balanced merges.
///
/// # Safety
///
/// (U3) `head` is a live circular list head and `cmp` is a valid comparison,
/// which the header requires with `nonnull(2,3)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn list_sort(
    priv_: *mut core::ffi::c_void,
    head: *mut list_head,
    cmp: list_cmp_func_t,
) {
    // SAFETY: (U3) `head` points to the caller's live list head.
    let mut list = unsafe { (*head).next };
    let mut pending: *mut list_head = ptr::null_mut();
    let mut count: usize = 0; /* Count of pending */

    // SAFETY: (U3) as above.
    if list == unsafe { (*head).prev } {
        /* Zero or one elements */
        return;
    }

    // Convert to a null-terminated singly-linked list.
    // SAFETY: (U3) `head->prev` is the last node of a non-empty list.
    unsafe { (*(*head).prev).next = ptr::null_mut() };

    // Data structure invariants:
    // - All lists are singly linked and null-terminated; prev
    //   pointers are not maintained.
    // - pending is a prev-linked "list of lists" of sorted
    //   sublists awaiting further merging.
    // - Each of the sorted sublists is power-of-two in size.
    // - Sublists are sorted by size and age, smallest & newest at front.
    // - There are zero to two sublists of each size.
    // - A pair of pending sublists are merged as soon as the number
    //   of following pending elements equals their size (i.e.
    //   each time count reaches an odd multiple of that size).
    //   That ensures each later final merge will be at worst 2:1.
    // - Each round consists of:
    //   - Merging the two sublists selected by the highest bit
    //     which flips when count is incremented, and
    //   - Adding an element from the input as a size-1 sublist.
    loop {
        let mut bits = count;
        let mut tail: *mut *mut list_head = &mut pending;

        // Find the least-significant clear bit in count
        while bits & 1 != 0 {
            // SAFETY: (U3) `*tail` is a pending sublist head, which the
            // invariants above say exists while the bit is set.
            tail = unsafe { &mut (**tail).prev };
            bits >>= 1;
        }
        // Do the indicated merge
        // `likely()` has no stable equivalent here; the branch is left plain.
        if bits != 0 {
            // SAFETY: (U3) both pending sublists exist, per the invariants.
            let (a, b) = unsafe { (*tail, (**tail).prev) };

            // SAFETY: (U3) `a` and `b` are non-null sublist heads; (U1) is
            // inside `merge()`.
            let a = unsafe { merge(priv_, cmp, b, a) };
            // Install the merged result in place of the inputs
            // SAFETY: (U3) `a` and `b` are live nodes.
            unsafe {
                (*a).prev = (*b).prev;
                *tail = a;
            }
        }

        // Move one element from input list to pending
        // SAFETY: (U3) `list` is a live node of the caller's list.
        unsafe {
            (*list).prev = pending;
            pending = list;
            list = (*list).next;
            (*pending).next = ptr::null_mut();
        }
        count += 1;

        if list.is_null() {
            break;
        }
    }

    // End of input; merge together all the pending lists.
    list = pending;
    // SAFETY: (U3) `pending` is a live sublist head.
    pending = unsafe { (*pending).prev };
    loop {
        // SAFETY: (U3) as above.
        let next = unsafe { (*pending).prev };

        if next.is_null() {
            break;
        }
        // SAFETY: (U3) both are non-null sublist heads.
        list = unsafe { merge(priv_, cmp, pending, list) };
        pending = next;
    }
    // The final merge, rebuilding prev links
    // SAFETY: (U3) `head` is the caller's head; `pending` and `list` are
    // non-null sublist heads.
    unsafe { merge_final(priv_, cmp, head, pending, list) };
}
