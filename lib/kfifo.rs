// SPDX-License-Identifier: GPL-2.0

//! A generic kernel FIFO implementation.
//!
//! The Rust implementation of `lib/kfifo.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/kfifo.h` is the ABI contract, and its
//! type-carrying macros stay in C. `lib/kfifo_ffi.c` exports these symbols
//! with the license of `lib/kfifo.c`.
//!
//! # Ordering
//!
//! kfifo is single-producer, single-consumer and takes no lock. The producer
//! copies data and then, after `smp_wmb()`, publishes it by advancing `in`.
//! The consumer reads `in` with **no barrier at all**, copies the data out,
//! and after `smp_wmb()` advances `out` to release the space.
//!
//! The C has no `smp_rmb()` and no `READ_ONCE`: the consumer's read of the
//! data is ordered after its read of `in` only by the address dependency
//! through `fifo->data + off`. Rust cannot express that dependency, so this
//! port keeps C's codegen rather than strengthening it - the indices are read
//! `Relaxed` and the barrier is the same `smp_wmb()` the C emits, through
//! `c_kfifo_smp_wmb()`. Using `Acquire` would be the safer Rust idiom and
//! would add a `dmb(ishld)` on arm64 that the C does not have; see
//! `units/kfifo.md`.
//!
//! `in` and `out` are plain `unsigned int` in C, shared between two threads
//! without annotation. That is a data race in Rust's model whatever the
//! hardware does, so they are `AtomicU32` here. The layout is unchanged.

use core::ffi::{c_int, c_uint, c_ulong, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

/// `gfp_t`, an `unsigned int` bitwise type in C.
#[allow(non_camel_case_types)]
pub type gfp_t = c_uint;

/// `dma_addr_t`: `u64` or `u32` by `CONFIG_ARCH_DMA_ADDR_T_64BIT`.
#[cfg(CONFIG_ARCH_DMA_ADDR_T_64BIT)]
#[allow(non_camel_case_types)]
pub type dma_addr_t = u64;
#[cfg(not(CONFIG_ARCH_DMA_ADDR_T_64BIT))]
#[allow(non_camel_case_types)]
pub type dma_addr_t = u32;

/// `DMA_MAPPING_ERROR`, `(~(dma_addr_t)0)` in `include/linux/dma-mapping.h`.
const DMA_MAPPING_ERROR: dma_addr_t = !0;

const EINVAL: c_int = 22;
const ENOMEM: c_int = 12;
const EFAULT: c_int = 14;

/// Mirror of `struct __kfifo`.
///
/// `in` and `out` are `AtomicU32` rather than `u32`: see the module comment.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct __kfifo {
    r#in: AtomicU32,
    out: AtomicU32,
    mask: c_uint,
    esize: c_uint,
    data: *mut c_void,
}

// `pahole -C __kfifo` over the BTF of the running kernel and over the DWARF of
// a tinyconfig SMP build. Five fixed fields, no config-dependent member.
//
// The 32-bit line is derived, not measured: four 4-byte integers followed by a
// 4-byte pointer, every member 4-aligned, so no padding is possible. No 32-bit
// build was made, as in `lib/list_sort.rs`.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(__kfifo, size = 24, align = 8,
    r#in @ 0, out @ 4, mask @ 8, esize @ 12, data @ 16);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(__kfifo, size = 20, align = 4,
    r#in @ 0, out @ 4, mask @ 8, esize @ 12, data @ 16);

unsafe extern "C" {
    /// `kmalloc_array_node()`, a macro over `alloc_hooks`.
    fn c_kfifo_kmalloc_array_node(n: usize, size: usize, gfp: gfp_t, node: c_int) -> *mut c_void;
    /// `kfree()` is a real exported function, so it needs no wrapper.
    fn kfree(objp: *const c_void);
    /// `copy_from_user()`, with its `access_ok` and object-size checks.
    fn c_kfifo_copy_from_user(to: *mut c_void, from: *const c_void, n: usize) -> usize;
    /// `copy_to_user()`, likewise.
    fn c_kfifo_copy_to_user(to: *mut c_void, from: *const c_void, n: usize) -> usize;
    /// `sg_set_buf(sgl + idx, ...)`: the index keeps `scatterlist` opaque.
    fn c_kfifo_sg_set_buf(sgl: *mut c_void, idx: c_uint, buf: *const c_void, len: c_uint);
    /// `sg_dma_address(sgl + idx) = addr`.
    fn c_kfifo_sg_set_dma_address(sgl: *mut c_void, idx: c_uint, addr: dma_addr_t);
    /// `sg_dma_len(sgl + idx) = len`.
    fn c_kfifo_sg_set_dma_len(sgl: *mut c_void, idx: c_uint, len: c_uint);
    /// `smp_wmb()`, the barrier this unit is about.
    fn c_kfifo_smp_wmb();
    /// `BUG_ON()`, which the record DMA paths use to reject `nents == 0`.
    fn c_kfifo_bug_on(condition: bool);
}

/// `smp_wmb()`.
///
/// Safe: a barrier has no preconditions, so this exposes a safe interface
/// over the FFI call.
#[inline]
fn smp_wmb() {
    // SAFETY: (U1) c_kfifo_smp_wmb() is a barrier and takes no arguments.
    unsafe { c_kfifo_smp_wmb() }
}

/// `is_power_of_2()`: `n - 1 < (n ^ (n - 1))`.
///
/// `wrapping_sub` because C's unsigned arithmetic wraps on zero where Rust's
/// `-` panics in a debug build.
#[inline]
fn is_power_of_2(n: usize) -> bool {
    n.wrapping_sub(1) < (n ^ n.wrapping_sub(1))
}

/// `__roundup_pow_of_two()`: `1UL << fls_long(n - 1)`.
///
/// The `roundup_pow_of_two()` macro picks a constant-folded path with
/// `__builtin_constant_p`; kfifo always passes a runtime value, so this is
/// the path the C takes.
#[inline]
fn roundup_pow_of_two(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    1usize << (usize::BITS - (n - 1).leading_zeros())
}

/// `__rounddown_pow_of_two()`: `1UL << ilog2(n)`.
#[inline]
fn rounddown_pow_of_two(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    1usize << (usize::BITS - 1 - n.leading_zeros())
}

impl __kfifo {
    /// The indices are read without ordering, as the C reads them: plain
    /// loads of `fifo->in` and `fifo->out`.
    #[inline]
    fn in_(&self) -> u32 {
        self.r#in.load(Ordering::Relaxed)
    }

    #[inline]
    fn out_(&self) -> u32 {
        self.out.load(Ordering::Relaxed)
    }

    /// `fifo->in += len`, after the barrier that publishes the data.
    #[inline]
    fn advance_in(&self, len: u32) {
        self.r#in
            .store(self.in_().wrapping_add(len), Ordering::Relaxed);
    }

    /// `fifo->out += len`.
    #[inline]
    fn advance_out(&self, len: u32) {
        self.out
            .store(self.out_().wrapping_add(len), Ordering::Relaxed);
    }

    /// `kfifo_unused()`: `(fifo->mask + 1) - (fifo->in - fifo->out)`.
    #[inline]
    fn unused(&self) -> u32 {
        (self.mask.wrapping_add(1)).wrapping_sub(self.in_().wrapping_sub(self.out_()))
    }

    /// `fifo->in - fifo->out`, the number of elements stored.
    #[inline]
    fn len(&self) -> u32 {
        self.in_().wrapping_sub(self.out_())
    }
}

/// Copy into the fifo at `off`, then the barrier that makes it visible.
///
/// # Safety
///
/// (U3) `fifo` is a live `__kfifo` with `data` covering `mask + 1` elements,
/// and `src` covers `len` elements, as the C contract requires.
unsafe fn kfifo_copy_in(fifo: &__kfifo, src: *const u8, len: u32, off: u32) {
    let mut size = fifo.mask.wrapping_add(1);
    let esize = fifo.esize;
    let mut len = len;
    let mut off = off & fifo.mask;

    if esize != 1 {
        off = off.wrapping_mul(esize);
        size = size.wrapping_mul(esize);
        len = len.wrapping_mul(esize);
    }
    let l = core::cmp::min(len, size - off);

    // SAFETY: (U3) both ranges are inside the caller's buffers, by the
    // bounds the caller computed from kfifo_unused().
    unsafe {
        ptr::copy_nonoverlapping(src, (fifo.data as *mut u8).add(off as usize), l as usize);
        ptr::copy_nonoverlapping(
            src.add(l as usize),
            fifo.data as *mut u8,
            (len - l) as usize,
        );
    }
    /*
     * make sure that the data in the fifo is up to date before
     * incrementing the fifo->in index counter
     */
    smp_wmb();
}

/// Copy out of the fifo at `off`, then the barrier before `out` advances.
///
/// # Safety
///
/// (U3) As `kfifo_copy_in`, with `dst` covering `len` elements.
unsafe fn kfifo_copy_out(fifo: &__kfifo, dst: *mut u8, len: u32, off: u32) {
    let mut size = fifo.mask.wrapping_add(1);
    let esize = fifo.esize;
    let mut len = len;
    let mut off = off & fifo.mask;

    if esize != 1 {
        off = off.wrapping_mul(esize);
        size = size.wrapping_mul(esize);
        len = len.wrapping_mul(esize);
    }
    let l = core::cmp::min(len, size - off);

    // SAFETY: (U3) both ranges are inside the caller's buffers.
    unsafe {
        ptr::copy_nonoverlapping((fifo.data as *const u8).add(off as usize), dst, l as usize);
        ptr::copy_nonoverlapping(
            fifo.data as *const u8,
            dst.add(l as usize),
            (len - l) as usize,
        );
    }
    /*
     * make sure that the data is copied before
     * incrementing the fifo->out index counter
     */
    smp_wmb();
}

/// # Safety
///
/// (U3) `fifo` points to a live `__kfifo`, per the C contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_alloc_node(
    fifo: *mut __kfifo,
    size: c_uint,
    esize: usize,
    gfp_mask: gfp_t,
    node: c_int,
) -> c_int {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &mut *fifo };

    /*
     * round up to the next power of 2, since our 'let the indices
     * wrap' technique works only in this case.
     */
    let size = roundup_pow_of_two(size as usize);

    fifo.r#in.store(0, Ordering::Relaxed);
    fifo.out.store(0, Ordering::Relaxed);
    fifo.esize = esize as c_uint;

    if size < 2 {
        fifo.data = ptr::null_mut();
        fifo.mask = 0;
        return -EINVAL;
    }

    // SAFETY: (U1) kmalloc_array_node() with the caller's gfp flags; it
    // returns null on failure, which is checked.
    fifo.data = unsafe { c_kfifo_kmalloc_array_node(size, esize, gfp_mask, node) };

    if fifo.data.is_null() {
        fifo.mask = 0;
        return -ENOMEM;
    }
    fifo.mask = size.wrapping_sub(1) as c_uint;

    0
}

/// # Safety
///
/// (U3) `fifo` points to a live `__kfifo` whose `data` came from
/// `__kfifo_alloc_node()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_free(fifo: *mut __kfifo) {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &mut *fifo };

    // SAFETY: (U1) kfree() of the buffer this unit allocated, or of null.
    unsafe { kfree(fifo.data) };
    fifo.r#in.store(0, Ordering::Relaxed);
    fifo.out.store(0, Ordering::Relaxed);
    fifo.esize = 0;
    fifo.data = ptr::null_mut();
    fifo.mask = 0;
}

/// # Safety
///
/// (U3) `fifo` points to a live `__kfifo`, and `buffer` covers `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_init(
    fifo: *mut __kfifo,
    buffer: *mut c_void,
    size: c_uint,
    esize: usize,
) -> c_int {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &mut *fifo };

    // `checked_div` and `wrapping_sub` below, not `/` and `-`: a division by
    // zero or an underflow is a panic in Rust where C simply computes, and a
    // panic landing pad in a kernel object is dead code the kernel cannot
    // unwind from. objtool sees those pads as a fall-through past the
    // function's end.
    let mut size = (size as usize).checked_div(esize).unwrap_or(0);

    if !is_power_of_2(size) {
        size = rounddown_pow_of_two(size);
    }

    fifo.r#in.store(0, Ordering::Relaxed);
    fifo.out.store(0, Ordering::Relaxed);
    fifo.esize = esize as c_uint;
    fifo.data = buffer;

    if size < 2 {
        fifo.mask = 0;
        return -EINVAL;
    }
    fifo.mask = size.wrapping_sub(1) as c_uint;

    0
}

/// # Safety
///
/// (U3) `fifo` is live and `buf` covers `len` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_in(fifo: *mut __kfifo, buf: *const c_void, len: c_uint) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let l = fifo.unused();
    let len = if len > l { l } else { len };

    // SAFETY: (U3) len is now bounded by the free space.
    unsafe { kfifo_copy_in(fifo, buf as *const u8, len, fifo.in_()) };
    fifo.advance_in(len);
    len
}

/// # Safety
///
/// (U3) `fifo` is live and `buf` covers `len` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_out_peek(
    fifo: *mut __kfifo,
    buf: *mut c_void,
    len: c_uint,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let l = fifo.len();
    let len = if len > l { l } else { len };

    // SAFETY: (U3) len is now bounded by what the fifo holds.
    unsafe { kfifo_copy_out(fifo, buf as *mut u8, len, fifo.out_()) };
    len
}

/// # Safety
///
/// (U3) `fifo` is live; `tail` is null or points to a writable `c_uint`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_out_linear(
    fifo: *mut __kfifo,
    tail: *mut c_uint,
    n: c_uint,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let size = fifo.mask.wrapping_add(1);
    let off = fifo.out_() & fifo.mask;

    if !tail.is_null() {
        // SAFETY: (U3) the caller passed a writable pointer.
        unsafe { *tail = off };
    }

    core::cmp::min(core::cmp::min(n, fifo.len()), size - off)
}

/// # Safety
///
/// (U3) `fifo` is live and `buf` covers `len` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_out(fifo: *mut __kfifo, buf: *mut c_void, len: c_uint) -> c_uint {
    // SAFETY: (U3) the caller's fifo and buffer.
    let len = unsafe { __kfifo_out_peek(fifo, buf, len) };
    // SAFETY: (U3) as above.
    unsafe { &*fifo }.advance_out(len);
    len
}

/// `DIV_ROUND_UP(n, d)`.
#[inline]
fn div_round_up(n: usize, d: usize) -> usize {
    // C's DIV_ROUND_UP divides; esize is never zero in a fifo the caller set
    // up, but a panic path here would be emitted whether or not it can run.
    match d {
        0 => 0,
        d => n.div_ceil(d),
    }
}

/// Copy from user space into the fifo at `off`, then the publishing barrier.
///
/// Returns the number of elements **not** copied, as the C does.
///
/// # Safety
///
/// (U3) `fifo` is live, `from` is a user pointer the caller obtained from a
/// syscall argument, and `copied` is writable.
unsafe fn kfifo_copy_from_user(
    fifo: &__kfifo,
    from: *const c_void,
    len: c_uint,
    off: c_uint,
    copied: *mut c_uint,
) -> usize {
    let mut size = fifo.mask.wrapping_add(1);
    let esize = fifo.esize;
    let mut len = len;
    let mut off = off & fifo.mask;

    if esize != 1 {
        off = off.wrapping_mul(esize);
        size = size.wrapping_mul(esize);
        len = len.wrapping_mul(esize);
    }
    let l = core::cmp::min(len, size - off);

    // SAFETY: (U1) copy_from_user() with a destination inside the fifo buffer
    // and a user source the caller supplied; it returns the byte count it
    // could not copy.
    let mut ret = unsafe {
        c_kfifo_copy_from_user(
            (fifo.data as *mut u8).add(off as usize) as *mut c_void,
            from,
            l as usize,
        )
    };
    if ret != 0 {
        ret = div_round_up(ret + (len - l) as usize, esize as usize);
    } else {
        // SAFETY: (U1) the wrapped part of the copy, as above.
        ret = unsafe {
            c_kfifo_copy_from_user(
                fifo.data,
                (from as *const u8).add(l as usize) as *const c_void,
                (len - l) as usize,
            )
        };
        if ret != 0 {
            ret = div_round_up(ret, esize as usize);
        }
    }
    /*
     * make sure that the data in the fifo is up to date before
     * incrementing the fifo->in index counter
     */
    smp_wmb();
    // SAFETY: (U3) the caller's out-parameter.
    unsafe { *copied = len - (ret as c_uint) * esize };
    /* return the number of elements which are not copied */
    ret
}

/// # Safety
///
/// (U3) `fifo` is live, `from` is a user pointer, `copied` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_from_user(
    fifo: *mut __kfifo,
    from: *const c_void,
    len: c_ulong,
    copied: *mut c_uint,
) -> c_int {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };
    let esize = fifo.esize;
    let mut len = len;

    if esize != 1 {
        len = len.checked_div(esize as c_ulong).unwrap_or(0);
    }

    let l = fifo.unused();
    if len > l as c_ulong {
        len = l as c_ulong;
    }

    // SAFETY: (U3) len is bounded by the free space; the pointers are the
    // caller's.
    let ret = unsafe { kfifo_copy_from_user(fifo, from, len as c_uint, fifo.in_(), copied) };
    let err = if ret != 0 {
        len = len.wrapping_sub(ret as c_ulong);
        -EFAULT
    } else {
        0
    };
    fifo.advance_in(len as u32);
    err
}

/// Copy out of the fifo at `off` to user space, then the barrier.
///
/// # Safety
///
/// (U3) As `kfifo_copy_from_user`, with `to` the user destination.
unsafe fn kfifo_copy_to_user(
    fifo: &__kfifo,
    to: *mut c_void,
    len: c_uint,
    off: c_uint,
    copied: *mut c_uint,
) -> usize {
    let mut size = fifo.mask.wrapping_add(1);
    let esize = fifo.esize;
    let mut len = len;
    let mut off = off & fifo.mask;

    if esize != 1 {
        off = off.wrapping_mul(esize);
        size = size.wrapping_mul(esize);
        len = len.wrapping_mul(esize);
    }
    let l = core::cmp::min(len, size - off);

    // SAFETY: (U1) copy_to_user() from inside the fifo buffer to the caller's
    // user destination.
    let mut ret = unsafe {
        c_kfifo_copy_to_user(
            to,
            (fifo.data as *const u8).add(off as usize) as *const c_void,
            l as usize,
        )
    };
    if ret != 0 {
        ret = div_round_up(ret + (len - l) as usize, esize as usize);
    } else {
        // SAFETY: (U1) the wrapped part of the copy.
        ret = unsafe {
            c_kfifo_copy_to_user(
                (to as *mut u8).add(l as usize) as *mut c_void,
                fifo.data,
                (len - l) as usize,
            )
        };
        if ret != 0 {
            ret = div_round_up(ret, esize as usize);
        }
    }
    /*
     * make sure that the data is copied before
     * incrementing the fifo->out index counter
     */
    smp_wmb();
    // SAFETY: (U3) the caller's out-parameter.
    unsafe { *copied = len - (ret as c_uint) * esize };
    /* return the number of elements which are not copied */
    ret
}

/// # Safety
///
/// (U3) `fifo` is live, `to` is a user pointer, `copied` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_to_user(
    fifo: *mut __kfifo,
    to: *mut c_void,
    len: c_ulong,
    copied: *mut c_uint,
) -> c_int {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };
    let esize = fifo.esize;
    let mut len = len;

    if esize != 1 {
        len = len.checked_div(esize as c_ulong).unwrap_or(0);
    }

    let l = fifo.len();
    if len > l as c_ulong {
        len = l as c_ulong;
    }

    // SAFETY: (U3) len is bounded by what the fifo holds.
    let ret = unsafe { kfifo_copy_to_user(fifo, to, len as c_uint, fifo.out_(), copied) };
    let err = if ret != 0 {
        len = len.wrapping_sub(ret as c_ulong);
        -EFAULT
    } else {
        0
    };
    fifo.advance_out(len as u32);
    err
}

/// One scatterlist entry, at index `idx` of the caller's array.
///
/// `struct scatterlist` is never dereferenced here: the wrappers take the
/// index, so this side needs neither its layout nor its size. See
/// `units/kfifo.md`.
///
/// # Safety
///
/// (U3) `fifo` is live and `sgl` points to an array of at least `idx + 1`
/// scatterlist entries, which the caller sized with `nents`.
unsafe fn setup_sgl_buf(
    fifo: &__kfifo,
    sgl: *mut c_void,
    idx: c_uint,
    data_offset: c_uint,
    nents: c_int,
    len: c_uint,
    dma: dma_addr_t,
) -> c_uint {
    if nents == 0 || len == 0 {
        return 0;
    }

    // SAFETY: (U3) data_offset is inside the fifo buffer.
    let buf = unsafe { (fifo.data as *const u8).add(data_offset as usize) };

    // SAFETY: (U1) sg_set_buf() on entry idx of the caller's array.
    unsafe { c_kfifo_sg_set_buf(sgl, idx, buf as *const c_void, len) };

    if dma != DMA_MAPPING_ERROR {
        // SAFETY: (U1) sg_dma_address() and sg_dma_len() on the same entry.
        unsafe {
            c_kfifo_sg_set_dma_address(sgl, idx, dma + data_offset as dma_addr_t);
            c_kfifo_sg_set_dma_len(sgl, idx, len);
        }
    }

    1
}

/// # Safety
///
/// (U3) As `setup_sgl_buf`, for up to two entries.
unsafe fn setup_sgl(
    fifo: &__kfifo,
    sgl: *mut c_void,
    nents: c_int,
    len: c_uint,
    off: c_uint,
    dma: dma_addr_t,
) -> c_uint {
    let mut size = fifo.mask.wrapping_add(1);
    let esize = fifo.esize;
    let mut len = len;
    let mut off = off & fifo.mask;

    if esize != 1 {
        off = off.wrapping_mul(esize);
        size = size.wrapping_mul(esize);
        len = len.wrapping_mul(esize);
    }
    let len_to_end = core::cmp::min(len, size - off);

    // SAFETY: (U3) entry 0, then entry n, of the caller's array.
    let n = unsafe { setup_sgl_buf(fifo, sgl, 0, off, nents, len_to_end, dma) };
    // SAFETY: (U3) as above; this is C's `sgl + n`, expressed as an index.
    let n2 = unsafe { setup_sgl_buf(fifo, sgl, n, 0, nents - n as c_int, len - len_to_end, dma) };

    n + n2
}

/// # Safety
///
/// (U3) `fifo` is live and `sgl` has room for `nents` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_dma_in_prepare(
    fifo: *mut __kfifo,
    sgl: *mut c_void,
    nents: c_int,
    len: c_uint,
    dma: dma_addr_t,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let l = fifo.unused();
    let len = if len > l { l } else { len };

    // SAFETY: (U3) len is bounded by the free space.
    unsafe { setup_sgl(fifo, sgl, nents, len, fifo.in_(), dma) }
}

/// # Safety
///
/// (U3) As `__kfifo_dma_in_prepare`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_dma_out_prepare(
    fifo: *mut __kfifo,
    sgl: *mut c_void,
    nents: c_int,
    len: c_uint,
    dma: dma_addr_t,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let l = fifo.len();
    let len = if len > l { l } else { len };

    // SAFETY: (U3) len is bounded by what the fifo holds.
    unsafe { setup_sgl(fifo, sgl, nents, len, fifo.out_(), dma) }
}

/// The largest record length `recsize` bytes can encode.
///
/// C writes `(1 << (recsize << 3)) - 1`. `recsize` is `sizeof(*rectype)` and
/// is 0, 1 or 2 in every caller; the shift is done in 64 bits here so that a
/// larger value cannot shift out of range.
#[unsafe(no_mangle)]
pub extern "C" fn __kfifo_max_r(len: c_uint, recsize: usize) -> c_uint {
    let max = (1u64
        .wrapping_shl((recsize as u32).wrapping_mul(8))
        .wrapping_sub(1)) as c_uint;

    if len > max {
        return max;
    }
    len
}

/// `BUG_ON()`.
///
/// Safe: it takes a condition and does not return when that condition is
/// true, so callers have nothing to uphold.
#[inline]
fn bug_on(condition: bool) {
    // SAFETY: (U1) the wrapper forwards to BUG_ON().
    unsafe { c_kfifo_bug_on(condition) }
}

/// `__kfifo_peek_n()`: the length of the next record.
///
/// `__KFIFO_PEEK` is a local macro in `lib/kfifo.c`, `(data)[(out) & (mask)]`,
/// so a record's length is one or two raw bytes at `out & mask`, low byte
/// first. No FFI is needed for it.
///
/// # Safety
///
/// (U3) `fifo` is live and holds at least one record.
unsafe fn kfifo_peek_n(fifo: &__kfifo, recsize: usize) -> c_uint {
    let mask = fifo.mask;
    let data = fifo.data as *const u8;

    // SAFETY: (U3) the index is masked into the buffer.
    let mut l = unsafe { *data.add((fifo.out_() & mask) as usize) } as c_uint;

    if recsize > 1 {
        // SAFETY: (U3) as above, for the second length byte.
        l |= (unsafe { *data.add((fifo.out_().wrapping_add(1) & mask) as usize) } as c_uint) << 8;
    }

    l
}

/// `__kfifo_poke_n()`: write the length of the record being added.
///
/// # Safety
///
/// (U3) `fifo` is live and has room for the record.
unsafe fn kfifo_poke_n(fifo: &__kfifo, n: c_uint, recsize: usize) {
    let mask = fifo.mask;
    let data = fifo.data as *mut u8;

    // SAFETY: (U3) the index is masked into the buffer.
    unsafe { *data.add((fifo.in_() & mask) as usize) = n as u8 };

    if recsize > 1 {
        // SAFETY: (U3) as above, for the second length byte.
        unsafe { *data.add((fifo.in_().wrapping_add(1) & mask) as usize) = (n >> 8) as u8 };
    }
}

/// # Safety
///
/// (U3) `fifo` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_len_r(fifo: *mut __kfifo, recsize: usize) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    unsafe { kfifo_peek_n(&*fifo, recsize) }
}

/// # Safety
///
/// (U3) `fifo` is live and `buf` covers `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_in_r(
    fifo: *mut __kfifo,
    buf: *const c_void,
    len: c_uint,
    recsize: usize,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    if len + recsize as c_uint > fifo.unused() {
        return 0;
    }

    // SAFETY: (U3) there is room for the record and its length prefix.
    unsafe { kfifo_poke_n(fifo, len, recsize) };

    // SAFETY: (U3) as above; the data goes after the length prefix.
    unsafe {
        kfifo_copy_in(
            fifo,
            buf as *const u8,
            len,
            fifo.in_().wrapping_add(recsize as c_uint),
        )
    };
    fifo.advance_in(len + recsize as c_uint);
    len
}

/// The shared body of the record read paths.
///
/// # Safety
///
/// (U3) `fifo` is live, holds a record, and `buf` covers `len` bytes.
unsafe fn kfifo_out_copy_r(
    fifo: &__kfifo,
    buf: *mut c_void,
    len: c_uint,
    recsize: usize,
    n: &mut c_uint,
) -> c_uint {
    // SAFETY: (U3) the fifo holds a record.
    *n = unsafe { kfifo_peek_n(fifo, recsize) };

    let len = if len > *n { *n } else { len };

    // SAFETY: (U3) len is bounded by the record's length.
    unsafe {
        kfifo_copy_out(
            fifo,
            buf as *mut u8,
            len,
            fifo.out_().wrapping_add(recsize as c_uint),
        )
    };
    len
}

/// # Safety
///
/// (U3) `fifo` is live and `buf` covers `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_out_peek_r(
    fifo: *mut __kfifo,
    buf: *mut c_void,
    len: c_uint,
    recsize: usize,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };
    let mut n: c_uint = 0;

    if fifo.in_() == fifo.out_() {
        return 0;
    }

    // SAFETY: (U3) the fifo is not empty.
    unsafe { kfifo_out_copy_r(fifo, buf, len, recsize, &mut n) }
}

/// # Safety
///
/// (U3) `fifo` is live; `tail` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_out_linear_r(
    fifo: *mut __kfifo,
    tail: *mut c_uint,
    n: c_uint,
    recsize: usize,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    if fifo.in_() == fifo.out_() {
        return 0;
    }

    if !tail.is_null() {
        // SAFETY: (U3) the caller passed a writable pointer.
        unsafe { *tail = fifo.out_().wrapping_add(recsize as c_uint) };
    }

    // SAFETY: (U3) the fifo is not empty.
    core::cmp::min(n, unsafe { kfifo_peek_n(fifo, recsize) })
}

/// # Safety
///
/// (U3) `fifo` is live and `buf` covers `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_out_r(
    fifo: *mut __kfifo,
    buf: *mut c_void,
    len: c_uint,
    recsize: usize,
) -> c_uint {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };
    let mut n: c_uint = 0;

    if fifo.in_() == fifo.out_() {
        return 0;
    }

    // SAFETY: (U3) the fifo is not empty.
    let len = unsafe { kfifo_out_copy_r(fifo, buf, len, recsize, &mut n) };
    fifo.advance_out(n + recsize as c_uint);
    len
}

/// # Safety
///
/// (U3) `fifo` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_skip_r(fifo: *mut __kfifo, recsize: usize) {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    // SAFETY: (U3) as above.
    let n = unsafe { kfifo_peek_n(fifo, recsize) };
    fifo.advance_out(n + recsize as c_uint);
}

/// # Safety
///
/// (U3) `fifo` is live, `from` is a user pointer, `copied` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_from_user_r(
    fifo: *mut __kfifo,
    from: *const c_void,
    len: c_ulong,
    copied: *mut c_uint,
    recsize: usize,
) -> c_int {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let len = __kfifo_max_r(len as c_uint, recsize);

    if len + recsize as c_uint > fifo.unused() {
        // SAFETY: (U3) the caller's out-parameter.
        unsafe { *copied = 0 };
        return 0;
    }

    // SAFETY: (U3) there is room for the record and its length prefix.
    unsafe { kfifo_poke_n(fifo, len, recsize) };

    // SAFETY: (U3) as above; (U1) is inside kfifo_copy_from_user().
    let ret = unsafe {
        kfifo_copy_from_user(
            fifo,
            from,
            len,
            fifo.in_().wrapping_add(recsize as c_uint),
            copied,
        )
    };
    if ret != 0 {
        // SAFETY: (U3) the caller's out-parameter.
        unsafe { *copied = 0 };
        return -EFAULT;
    }
    fifo.advance_in(len + recsize as c_uint);
    0
}

/// # Safety
///
/// (U3) `fifo` is live, `to` is a user pointer, `copied` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_to_user_r(
    fifo: *mut __kfifo,
    to: *mut c_void,
    len: c_ulong,
    copied: *mut c_uint,
    recsize: usize,
) -> c_int {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    if fifo.in_() == fifo.out_() {
        // SAFETY: (U3) the caller's out-parameter.
        unsafe { *copied = 0 };
        return 0;
    }

    // SAFETY: (U3) the fifo is not empty.
    let n = unsafe { kfifo_peek_n(fifo, recsize) };
    let len = if len > n as c_ulong {
        n as c_ulong
    } else {
        len
    };

    // SAFETY: (U3) len is bounded by the record; (U1) is inside the callee.
    let ret = unsafe {
        kfifo_copy_to_user(
            fifo,
            to,
            len as c_uint,
            fifo.out_().wrapping_add(recsize as c_uint),
            copied,
        )
    };
    if ret != 0 {
        // SAFETY: (U3) the caller's out-parameter.
        unsafe { *copied = 0 };
        return -EFAULT;
    }
    fifo.advance_out(n + recsize as c_uint);
    0
}

/// # Safety
///
/// (U3) `fifo` is live and `sgl` has room for `nents` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_dma_in_prepare_r(
    fifo: *mut __kfifo,
    sgl: *mut c_void,
    nents: c_int,
    len: c_uint,
    recsize: usize,
    dma: dma_addr_t,
) -> c_uint {
    bug_on(nents == 0);

    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let len = __kfifo_max_r(len, recsize);

    if len + recsize as c_uint > fifo.unused() {
        return 0;
    }

    // SAFETY: (U3) there is room for the record.
    unsafe {
        setup_sgl(
            fifo,
            sgl,
            nents,
            len,
            fifo.in_().wrapping_add(recsize as c_uint),
            dma,
        )
    }
}

/// # Safety
///
/// (U3) `fifo` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_dma_in_finish_r(fifo: *mut __kfifo, len: c_uint, recsize: usize) {
    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let len = __kfifo_max_r(len, recsize);
    // SAFETY: (U3) the caller prepared room for this record.
    unsafe { kfifo_poke_n(fifo, len, recsize) };
    fifo.advance_in(len + recsize as c_uint);
}

/// # Safety
///
/// (U3) `fifo` is live and `sgl` has room for `nents` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __kfifo_dma_out_prepare_r(
    fifo: *mut __kfifo,
    sgl: *mut c_void,
    nents: c_int,
    len: c_uint,
    recsize: usize,
    dma: dma_addr_t,
) -> c_uint {
    bug_on(nents == 0);

    // SAFETY: (U3) the caller's fifo.
    let fifo = unsafe { &*fifo };

    let len = __kfifo_max_r(len, recsize);

    if len + recsize as c_uint > fifo.len() {
        return 0;
    }

    // SAFETY: (U3) the record is present.
    unsafe {
        setup_sgl(
            fifo,
            sgl,
            nents,
            len,
            fifo.out_().wrapping_add(recsize as c_uint),
            dma,
        )
    }
}
