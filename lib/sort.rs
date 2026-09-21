// SPDX-License-Identifier: GPL-2.0

//! A fast, small, non-recursive O(n log n) sort for the Linux kernel, replacing
//! `lib/sort.c` in place.
//!
//! This performs n*log2(n) + 0.37*n + o(n) comparisons on average,
//! and 1.5*n*log2(n) + O(n) in the (very contrived) worst case.
//!
//! Quicksort manages n*log2(n) - 1.26*n for random inputs (1.63*n
//! better) at the expense of stack usage and much larger code to avoid
//! quicksort's O(n^2) worst case.
//!
//! The port keeps the C's control flow and its arithmetic: the offsets are
//! `size_t` values that wrap, so the loops use wrapping operations. The sort is
//! not stable, so which of two equal elements ends up first depends on the exact
//! sequence of comparisons and swaps, and the differential test checks that.
//!
//! The C selects a swap function with small integers that "can't be confused with
//! a pointer". Here that is [`Swap`], and the wrapper of `sort()` is [`Cmp`] and
//! [`Swap`] variants, not a struct passed as `priv`.

use core::ffi::{c_int, c_void};
use core::ptr;

/// `cmp_r_func_t`.
#[allow(non_camel_case_types)]
type cmp_r_func_t = unsafe extern "C" fn(*const c_void, *const c_void, *const c_void) -> c_int;
/// `cmp_func_t`.
#[allow(non_camel_case_types)]
type cmp_func_t = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;
/// `swap_r_func_t`.
#[allow(non_camel_case_types)]
type swap_r_func_t = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *const c_void);
/// `swap_func_t`.
#[allow(non_camel_case_types)]
type swap_func_t = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int);

unsafe extern "C" {
    /// `cond_resched()`, a macro.
    fn c_sort_cond_resched();
}

/// Reads a `T` at `p`, which need not be aligned.
#[inline]
fn read<T>(p: *const u8) -> T {
    // SAFETY: (U3) `p` is inside an element of the array being sorted, which
    // the caller owns for the call and which has at least size_of::<T>() bytes
    // left at `p`: the swaps step through an element in chunks of that size.
    unsafe { ptr::read_unaligned(p.cast::<T>()) }
}

/// Writes a `T` at `p`, which need not be aligned.
#[inline]
fn write<T>(p: *mut u8, v: T) {
    // SAFETY: (U3) as read(): `p` is inside an element of the array being sorted.
    unsafe { ptr::write_unaligned(p.cast::<T>(), v) }
}

/// is_aligned - is this pointer & size okay for word-wide copying?
/// @base: pointer to data
/// @size: size of each element
/// @align: required alignment (typically 4 or 8)
///
/// Returns true if elements can be copied using word loads and stores.
/// The size must be a multiple of the alignment, and the base address must
/// be if we do not have CONFIG_HAVE_EFFICIENT_UNALIGNED_ACCESS.
///
/// For some reason, gcc doesn't know to optimize "if (a & mask || b & mask)"
/// to "if ((a | b) & mask)", so we do that by hand.
#[inline(always)]
fn is_aligned(base: *const u8, size: usize, align: u8) -> bool {
    // Only the configs without efficient unaligned access add the base address.
    #[allow(unused_mut)]
    let mut lsbits = size as u8;

    let _ = base;
    #[cfg(not(CONFIG_HAVE_EFFICIENT_UNALIGNED_ACCESS))]
    {
        lsbits |= base as usize as u8;
    }
    (lsbits & (align - 1)) == 0
}

/// swap_words_32 - swap two elements in 32-bit chunks
/// @a: pointer to the first element to swap
/// @b: pointer to the second element to swap
/// @n: element size (must be a multiple of 4)
///
/// Exchange the two objects in memory.  This exploits base+index addressing,
/// which basically all CPUs have, to minimize loop overhead computations.
fn swap_words_32(a: *mut u8, b: *mut u8, mut n: usize) {
    loop {
        n -= 4;
        let t: u32 = read(a.wrapping_add(n));
        write(a.wrapping_add(n), read::<u32>(b.wrapping_add(n)));
        write(b.wrapping_add(n), t);
        if n == 0 {
            break;
        }
    }
}

/// swap_words_64 - swap two elements in 64-bit chunks
/// @a: pointer to the first element to swap
/// @b: pointer to the second element to swap
/// @n: element size (must be a multiple of 8)
///
/// Exchange the two objects in memory.  This exploits base+index
/// addressing, which basically all CPUs have, to minimize loop overhead
/// computations.
///
/// We'd like to use 64-bit loads if possible.  If they're not, emulating
/// one requires base+index+4 addressing which x86 has but most other
/// processors do not.  If CONFIG_64BIT, we definitely have 64-bit loads,
/// but it's possible to have 64-bit loads without 64-bit pointers (e.g.
/// x32 ABI).  Are there any cases the kernel needs to worry about?
fn swap_words_64(a: *mut u8, b: *mut u8, mut n: usize) {
    loop {
        #[cfg(target_pointer_width = "64")]
        {
            n -= 8;
            let t: u64 = read(a.wrapping_add(n));
            write(a.wrapping_add(n), read::<u64>(b.wrapping_add(n)));
            write(b.wrapping_add(n), t);
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            /* Use two 32-bit transfers to avoid base+index+4 addressing */
            n -= 4;
            let t: u32 = read(a.wrapping_add(n));
            write(a.wrapping_add(n), read::<u32>(b.wrapping_add(n)));
            write(b.wrapping_add(n), t);

            n -= 4;
            let t: u32 = read(a.wrapping_add(n));
            write(a.wrapping_add(n), read::<u32>(b.wrapping_add(n)));
            write(b.wrapping_add(n), t);
        }
        if n == 0 {
            break;
        }
    }
}

/// swap_bytes - swap two elements a byte at a time
/// @a: pointer to the first element to swap
/// @b: pointer to the second element to swap
/// @n: element size
///
/// This is the fallback if alignment doesn't allow using larger chunks.
fn swap_bytes(a: *mut u8, b: *mut u8, mut n: usize) {
    loop {
        n -= 1;
        let t: u8 = read(a.wrapping_add(n));
        write(a.wrapping_add(n), read::<u8>(b.wrapping_add(n)));
        write(b.wrapping_add(n), t);
        if n == 0 {
            break;
        }
    }
}

/// How two elements are exchanged: the C's `swap_r_func_t` with its four
/// integers that can't be confused with a pointer (`SWAP_WORDS_64`,
/// `SWAP_WORDS_32`, `SWAP_BYTES`, `SWAP_WRAPPER`), and the function pointers.
#[derive(Clone, Copy)]
enum Swap {
    Words64,
    Words32,
    Bytes,
    /// A `swap_r_func_t` and the `priv` it takes.
    Plain(swap_r_func_t, *const c_void),
    /// The `swap_func_t` of `sort()`, which takes no `priv`.
    Wrapper(swap_func_t),
}

/// How two elements are compared: `cmp_r_func_t` and its `priv`, or the
/// `cmp_func_t` of `sort()`, the C's `_CMP_WRAPPER`.
#[derive(Clone, Copy)]
enum Cmp {
    Plain(cmp_r_func_t, *const c_void),
    Wrapper(cmp_func_t),
}

/// The function pointer is last to make tail calls most efficient if the
/// compiler decides not to inline this function.
fn do_swap(a: *mut u8, b: *mut u8, size: usize, swap: Swap) {
    match swap {
        Swap::Wrapper(swap) => {
            // SAFETY: (U1) the caller's swap function, called with two elements
            // of the array and their size, as sort() documents.
            unsafe { swap(a.cast(), b.cast(), size as c_int) }
        }
        Swap::Words64 => swap_words_64(a, b, size),
        Swap::Words32 => swap_words_32(a, b, size),
        Swap::Bytes => swap_bytes(a, b, size),
        Swap::Plain(swap, priv_) => {
            // SAFETY: (U1) as above, with the caller's `priv`.
            unsafe { swap(a.cast(), b.cast(), size as c_int, priv_) }
        }
    }
}

fn do_cmp(a: *const u8, b: *const u8, cmp: Cmp) -> c_int {
    match cmp {
        // SAFETY: (U1) the caller's comparison function, called with two elements
        // of the array, as sort() documents.
        Cmp::Wrapper(cmp) => unsafe { cmp(a.cast(), b.cast()) },
        // SAFETY: (U1) as above, with the caller's `priv`.
        Cmp::Plain(cmp, priv_) => unsafe { cmp(a.cast(), b.cast(), priv_) },
    }
}

/// parent - given the offset of the child, find the offset of the parent.
/// @i: the offset of the heap element whose parent is sought.  Non-zero.
/// @lsbit: a precomputed 1-bit mask, equal to "size & -size"
/// @size: size of each element
///
/// In terms of array indexes, the parent of element j = @i/@size is simply
/// (j-1)/2.  But when working in byte offsets, we can't use implicit
/// truncation of integer divides.
///
/// Fortunately, we only need one bit of the quotient, not the full divide.
/// @size has a least significant bit.  That bit will be clear if @i is
/// an even multiple of @size, and set if it's an odd multiple.
///
/// Logically, we're doing "if (i & lsbit) i -= size;", but since the
/// branch is unpredictable, it's done with a bit of clever branch-free
/// code instead.
#[inline(always)]
fn parent(i: usize, lsbit: u32, size: usize) -> usize {
    let mut i = i.wrapping_sub(size);
    i = i.wrapping_sub(size & (i & lsbit as usize).wrapping_neg());
    i / 2
}

fn sort_r_impl(
    base: *mut u8,
    num: usize,
    size: usize,
    cmp_func: Cmp,
    swap_func: Option<Swap>,
    may_schedule: bool,
) {
    /* pre-scale counters for performance */
    let mut n = num.wrapping_mul(size);
    let mut a = (num / 2).wrapping_mul(size);
    /* Used to find parent. The C stores `size & -size` in an unsigned int. */
    let lsbit = (size & size.wrapping_neg()) as u32;
    let mut shift = 0;

    /* num < 2 || size == 0 */
    if a == 0 {
        return;
    }

    /* called from 'sort' without swap function, let's pick the default */
    let swap_func = swap_func.unwrap_or(if is_aligned(base, size, 8) {
        Swap::Words64
    } else if is_aligned(base, size, 4) {
        Swap::Words32
    } else {
        Swap::Bytes
    });

    /*
     * Loop invariants:
     * 1. elements [a,n) satisfy the heap property (compare greater than
     *    all of their children),
     * 2. elements [n,num*size) are sorted, and
     * 3. a <= b <= c <= d <= n (whenever they are valid).
     */
    loop {
        if a != 0 {
            /* Building heap: sift down a */
            a = a.wrapping_sub(size << shift);
        } else if n > size.wrapping_mul(3) {
            /* Sorting: Extract two largest elements */
            n = n.wrapping_sub(size);
            do_swap(base, base.wrapping_add(n), size, swap_func);
            shift = usize::from(
                do_cmp(
                    base.wrapping_add(size),
                    base.wrapping_add(2usize.wrapping_mul(size)),
                    cmp_func,
                ) <= 0,
            );
            a = size << shift;
            n = n.wrapping_sub(size);
            do_swap(base.wrapping_add(a), base.wrapping_add(n), size, swap_func);
        } else {
            /* Sort complete */
            break;
        }

        /*
         * Sift element at "a" down into heap.  This is the
         * "bottom-up" variant, which significantly reduces
         * calls to cmp_func(): we find the sift-down path all
         * the way to the leaves (one compare per level), then
         * backtrack to find where to insert the target element.
         *
         * Because elements tend to sift down close to the leaves,
         * this uses fewer compares than doing two per level
         * on the way down.  (A bit more than half as many on
         * average, 3/4 worst-case.)
         */
        let mut b = a;
        let mut c;
        let mut d;
        loop {
            c = b.wrapping_mul(2).wrapping_add(size);
            d = c.wrapping_add(size);
            if d >= n {
                break;
            }
            b = if do_cmp(base.wrapping_add(c), base.wrapping_add(d), cmp_func) > 0 {
                c
            } else {
                d
            };
        }
        /* Special case last leaf with no sibling */
        if d == n {
            b = c;
        }

        /* Now backtrack from "b" to the correct location for "a" */
        while b != a && do_cmp(base.wrapping_add(a), base.wrapping_add(b), cmp_func) >= 0 {
            b = parent(b, lsbit, size);
        }
        /* Where "a" belongs */
        c = b;
        /* Shift it into place */
        while b != a {
            b = parent(b, lsbit, size);
            do_swap(base.wrapping_add(b), base.wrapping_add(c), size, swap_func);
        }

        if may_schedule {
            // SAFETY: (U1) c_sort_cond_resched() takes no arguments and only calls
            // cond_resched(), which the non-atomic variants may do.
            unsafe { c_sort_cond_resched() };
        }
    }

    n = n.wrapping_sub(size);
    do_swap(base, base.wrapping_add(n), size, swap_func);
    if n == size.wrapping_mul(2) && do_cmp(base, base.wrapping_add(size), cmp_func) > 0 {
        do_swap(base, base.wrapping_add(size), size, swap_func);
    }
}

/// sort_r - sort an array of elements
/// @base: pointer to data to sort
/// @num: number of elements
/// @size: size of each element
/// @cmp_func: pointer to comparison function
/// @swap_func: pointer to swap function or NULL
/// @priv: third argument passed to comparison function
///
/// This function does a heapsort on the given array.  You may provide
/// a swap_func function if you need to do something more than a memory
/// copy (e.g. fix up pointers or auxiliary data), but the built-in swap
/// avoids a slow retpoline and so is significantly faster.
///
/// The comparison function must adhere to specific mathematical
/// properties to ensure correct and stable sorting:
/// - Antisymmetry: cmp_func(a, b) must return the opposite sign of
///   cmp_func(b, a).
/// - Transitivity: if cmp_func(a, b) <= 0 and cmp_func(b, c) <= 0, then
///   cmp_func(a, c) <= 0.
///
/// Sorting time is O(n log n) both on average and worst-case. While
/// quicksort is slightly faster on average, it suffers from exploitable
/// O(n*n) worst-case behavior and extra memory requirements that make
/// it less suitable for kernel use.
///
/// # Safety
///
/// `base` points to `num` elements of `size` bytes that the caller owns for the
/// call. `cmp_func` is not NULL and, like `swap_func` if it is not NULL, may be
/// called with any two of them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sort_r(
    base: *mut c_void,
    num: usize,
    size: usize,
    cmp_func: cmp_r_func_t,
    swap_func: Option<swap_r_func_t>,
    priv_: *const c_void,
) {
    sort_r_impl(
        base.cast(),
        num,
        size,
        Cmp::Plain(cmp_func, priv_),
        swap_func.map(|swap| Swap::Plain(swap, priv_)),
        false,
    );
}

/// sort_r_nonatomic - sort an array of elements, with cond_resched
/// @base: pointer to data to sort
/// @num: number of elements
/// @size: size of each element
/// @cmp_func: pointer to comparison function
/// @swap_func: pointer to swap function or NULL
/// @priv: third argument passed to comparison function
///
/// Same as sort_r, but preferred for larger arrays as it does a periodic
/// cond_resched().
///
/// # Safety
///
/// As for [`sort_r()`], and the caller may sleep.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sort_r_nonatomic(
    base: *mut c_void,
    num: usize,
    size: usize,
    cmp_func: cmp_r_func_t,
    swap_func: Option<swap_r_func_t>,
    priv_: *const c_void,
) {
    sort_r_impl(
        base.cast(),
        num,
        size,
        Cmp::Plain(cmp_func, priv_),
        swap_func.map(|swap| Swap::Plain(swap, priv_)),
        true,
    );
}

/// # Safety
///
/// As for [`sort_r()`], with a comparison and a swap function that take no `priv`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sort(
    base: *mut c_void,
    num: usize,
    size: usize,
    cmp_func: cmp_func_t,
    swap_func: Option<swap_func_t>,
) {
    sort_r_impl(
        base.cast(),
        num,
        size,
        Cmp::Wrapper(cmp_func),
        swap_func.map(Swap::Wrapper),
        false,
    );
}

/// # Safety
///
/// As for [`sort()`], and the caller may sleep.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sort_nonatomic(
    base: *mut c_void,
    num: usize,
    size: usize,
    cmp_func: cmp_func_t,
    swap_func: Option<swap_func_t>,
) {
    sort_r_impl(
        base.cast(),
        num,
        size,
        Cmp::Wrapper(cmp_func),
        swap_func.map(Swap::Wrapper),
        true,
    );
}
