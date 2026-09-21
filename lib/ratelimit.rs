// SPDX-License-Identifier: GPL-2.0

//! Rate limiting, replacing `lib/ratelimit.c` in place.
//!
//! `struct ratelimit_state` embeds a `raw_spinlock_t`, whose size depends on
//! the configuration: zero on UP, four bytes on SMP and 64 with lock
//! debugging. Every later field moves with it, so no single set of literals
//! describes the struct. Kbuild measures the layout with the C compiler for
//! the active configuration (`kernel/port-layout.c`) and the mirror below
//! asserts exactly those values. The lock stays opaque and is taken through
//! `lib/ratelimit_ffi.c`.
//!
//! C code keeps reading and writing the state: `ratelimit_state_init()`,
//! `ratelimit_set_flags()` and the `.proc_handler()` of `net_ratelimit_state`
//! all touch fields without this unit's knowledge. Every field is therefore
//! interior-mutable, and a plain C access is expressed as a `Relaxed` atomic
//! one, which is what `READ_ONCE()` and `WRITE_ONCE()` map to.
//!
//! The algorithm is generic over [`Ratelimit`], which the live C state
//! implements here. The harness implements it on loom's atomics, so its models
//! run the code of this file rather than a copy of it.

use core::ffi::{c_char, c_int, c_ulong, c_void};
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use crate::port_layout as layout;
use crate::refcount::atomic_t;

/// `RATELIMIT_MSG_ON_RELEASE`, `BIT(0)`.
pub(crate) const RATELIMIT_MSG_ON_RELEASE: u32 = 1 << 0;
/// `RATELIMIT_INITIALIZED`, `BIT(1)`.
pub(crate) const RATELIMIT_INITIALIZED: u32 = 1 << 1;

/// `struct ratelimit_state` from `include/linux/ratelimit_types.h`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct ratelimit_state {
    /// `raw_spinlock_t`, opaque: its size is the only thing Rust needs.
    #[allow(dead_code)]
    lock: kr::OpaqueBytes<{ layout::PORT_RATELIMIT_LOCK_SIZE }>,
    interval: AtomicI32,
    burst: AtomicI32,
    rs_n_left: atomic_t,
    /// Only C touches this one, through `ratelimit_state_inc_miss()` and
    /// `ratelimit_state_reset_miss()`.
    #[allow(dead_code)]
    missed: atomic_t,
    flags: AtomicU32,
    begin: AtomicUsize,
}

kr::static_assert_layout!(
    ratelimit_state,
    size = layout::PORT_RATELIMIT_SIZE,
    align = layout::PORT_RATELIMIT_ALIGN,
    lock @ layout::PORT_RATELIMIT_LOCK,
    interval @ layout::PORT_RATELIMIT_INTERVAL,
    burst @ layout::PORT_RATELIMIT_BURST,
    rs_n_left @ layout::PORT_RATELIMIT_RS_N_LEFT,
    missed @ layout::PORT_RATELIMIT_MISSED,
    flags @ layout::PORT_RATELIMIT_FLAGS,
    begin @ layout::PORT_RATELIMIT_BEGIN,
);
// `begin` is an `unsigned long`, and `jiffies` is read as one.
kr::static_assert!(size_of::<c_ulong>() == size_of::<usize>());
kr::static_assert!(RATELIMIT_MSG_ON_RELEASE as usize == layout::PORT_RATELIMIT_MSG_ON_RELEASE);
kr::static_assert!(RATELIMIT_INITIALIZED as usize == layout::PORT_RATELIMIT_INITIALIZED);

unsafe extern "C" {
    /// `raw_spin_trylock_irqsave(&rs->lock, *flags)`.
    fn c_ratelimit_trylock_irqsave(rs: *mut c_void, flags: *mut c_ulong) -> c_int;
    /// `raw_spin_unlock_irqrestore(&rs->lock, flags)`.
    fn c_ratelimit_unlock_irqrestore(rs: *mut c_void, flags: c_ulong);
    /// `ratelimit_state_inc_miss()`, a static inline of `<linux/ratelimit.h>`.
    fn c_ratelimit_inc_miss(rs: *mut c_void);
    /// `ratelimit_state_reset_miss()`, likewise.
    fn c_ratelimit_reset_miss(rs: *mut c_void) -> c_int;
    /// The `WARN_ONCE()` on a negative interval or burst.
    fn c_ratelimit_warn_negative(interval: c_int, burst: c_int);
    /// `printk_deferred()` of the suppressed-callbacks message.
    fn c_ratelimit_printk_suppressed(func: *const c_char, missed: c_int);

    /// The tick counter, `extern unsigned long volatile jiffies`.
    #[allow(non_upper_case_globals)]
    static jiffies: c_ulong;
}

/// What `ratelimit()` reads, writes and calls: the fields of the state, the tick
/// counter, the raw spinlock and the C helpers.
///
/// Implemented here for the live C state. The harness implements it on loom's
/// atomics, so its models run `ratelimit()` itself, as `lib/errseq.rs` and
/// `lib/refcount.rs` do for theirs. Each method names the C operation it
/// stands for.
pub(crate) trait Ratelimit {
    /// `READ_ONCE(rs->interval)`.
    fn interval(&self) -> i32;
    /// `READ_ONCE(rs->burst)`, and the plain reads of it.
    fn burst(&self) -> i32;
    /// `READ_ONCE(rs->flags)`, and the plain reads of it.
    fn flags(&self) -> u32;
    /// The plain stores to `rs->flags`.
    fn set_flags(&self, flags: u32);
    /// `atomic_read(&rs->rs_n_left)`.
    fn n_left(&self) -> i32;
    /// `atomic_set(&rs->rs_n_left, n)`.
    fn set_n_left(&self, n: i32);
    /// `atomic_dec_return(&rs->rs_n_left)`: fully ordered, the new value.
    fn dec_return_n_left(&self) -> i32;
    /// The plain reads of `rs->begin`, which C does under the lock.
    fn begin(&self) -> usize;
    /// The plain stores to `rs->begin`, under the lock.
    fn set_begin(&self, begin: usize);
    /// `READ_ONCE(jiffies)`.
    fn jiffies(&self) -> usize;
    /// `raw_spin_trylock_irqsave(&rs->lock, *flags)`.
    fn trylock_irqsave(&self, flags: &mut c_ulong) -> bool;
    /// `raw_spin_unlock_irqrestore(&rs->lock, flags)`.
    fn unlock_irqrestore(&self, flags: c_ulong);
    /// `ratelimit_state_inc_miss()`.
    fn inc_miss(&self);
    /// `ratelimit_state_reset_miss()`.
    fn reset_miss(&self) -> c_int;
    /// The `WARN_ONCE()` on a negative interval or burst.
    fn warn_negative(&self, interval: c_int, burst: c_int);
    /// `printk_deferred()` of the suppressed-callbacks message.
    fn printk_suppressed(&self, missed: c_int);
}

/// The state that a call of `___ratelimit()` runs on: the caller's
/// `ratelimit_state`, and the `func` that its message names.
struct Live<'a> {
    /// The pointer the C wrappers take, kept whole for their provenance.
    rs: *mut ratelimit_state,
    state: &'a ratelimit_state,
    func: *const c_char,
}

impl Ratelimit for Live<'_> {
    #[inline]
    fn interval(&self) -> i32 {
        self.state.interval.load(Ordering::Relaxed)
    }

    #[inline]
    fn burst(&self) -> i32 {
        self.state.burst.load(Ordering::Relaxed)
    }

    #[inline]
    fn flags(&self) -> u32 {
        self.state.flags.load(Ordering::Relaxed)
    }

    #[inline]
    fn set_flags(&self, flags: u32) {
        self.state.flags.store(flags, Ordering::Relaxed)
    }

    #[inline]
    fn n_left(&self) -> i32 {
        self.state.rs_n_left.counter().load(Ordering::Relaxed)
    }

    #[inline]
    fn set_n_left(&self, n: i32) {
        self.state.rs_n_left.counter().store(n, Ordering::Relaxed)
    }

    #[inline]
    fn dec_return_n_left(&self) -> i32 {
        self.state
            .rs_n_left
            .counter()
            .fetch_sub(1, Ordering::SeqCst)
            .wrapping_sub(1)
    }

    #[inline]
    fn begin(&self) -> usize {
        self.state.begin.load(Ordering::Relaxed)
    }

    #[inline]
    fn set_begin(&self, begin: usize) {
        self.state.begin.store(begin, Ordering::Relaxed)
    }

    #[inline]
    fn jiffies(&self) -> usize {
        // SAFETY: (U3) jiffies is a live C volatile unsigned long that the
        // timekeeping tick updates, readable from any context, as the C code
        // reads it.
        unsafe { ptr::read_volatile(&raw const jiffies) as usize }
    }

    #[inline]
    fn trylock_irqsave(&self, flags: &mut c_ulong) -> bool {
        // SAFETY: (U1) the C wrapper only forwards to
        // raw_spin_trylock_irqsave() on the lock of the caller's live
        // ratelimit_state, and stores the saved interrupt state in `flags`,
        // which is a live c_ulong.
        unsafe { c_ratelimit_trylock_irqsave(self.rs.cast(), flags) != 0 }
    }

    #[inline]
    fn unlock_irqrestore(&self, flags: c_ulong) {
        // SAFETY: (U1) this thread holds the lock, taken by
        // trylock_irqsave() with the same `flags`.
        unsafe { c_ratelimit_unlock_irqrestore(self.rs.cast(), flags) }
    }

    #[inline]
    fn inc_miss(&self) {
        // SAFETY: (U1) the C wrapper only does atomic_inc(&rs->missed) on the
        // caller's live ratelimit_state.
        unsafe { c_ratelimit_inc_miss(self.rs.cast()) }
    }

    #[inline]
    fn reset_miss(&self) -> c_int {
        // SAFETY: (U1) the C wrapper only does atomic_xchg_relaxed(&rs->missed,
        // 0) on the caller's live ratelimit_state.
        unsafe { c_ratelimit_reset_miss(self.rs.cast()) }
    }

    #[inline]
    fn warn_negative(&self, interval: c_int, burst: c_int) {
        // SAFETY: (U1) c_ratelimit_warn_negative() takes the two values and
        // only WARN_ONCE()s.
        unsafe { c_ratelimit_warn_negative(interval, burst) }
    }

    #[inline]
    fn printk_suppressed(&self, missed: c_int) {
        // SAFETY: (U1) c_ratelimit_printk_suppressed() takes the caller's
        // NUL-terminated `func` and only printk_deferred()s it with the
        // count.
        unsafe { c_ratelimit_printk_suppressed(self.func, missed) }
    }
}

/// `time_is_before_jiffies(a)`, that is `time_after(jiffies, a)`:
/// `(long)(a - jiffies) < 0`.
fn time_is_before_jiffies<R: Ratelimit>(rs: &R, a: usize) -> bool {
    (a.wrapping_sub(rs.jiffies()) as isize) < 0
}

/// The `nolock_ret:` label of `___ratelimit()`.
fn nolock_ret<R: Ratelimit>(rs: &R, ret: c_int) -> c_int {
    if ret == 0 {
        rs.inc_miss();
    }

    ret
}

/// The `unlock_ret:` label of `___ratelimit()`, which falls through to
/// `nolock_ret:`.
fn unlock_ret<R: Ratelimit>(rs: &R, flags: c_ulong, ret: c_int) -> c_int {
    rs.unlock_irqrestore(flags);
    nolock_ret(rs, ret)
}

/// The body of `___ratelimit()`, on any [`Ratelimit`] state.
pub(crate) fn ratelimit<R: Ratelimit>(rs: &R) -> c_int {
    // Paired with WRITE_ONCE() in .proc_handler().
    // Changing two values separately could be inconsistent
    // and some message could be lost.  (See: net_ratelimit_state).
    let interval = rs.interval();
    let burst = rs.burst();
    let mut flags: c_ulong = 0;
    let mut ret = 0;

    /*
     * Zero interval says never limit, otherwise, non-positive burst
     * says always limit.
     */
    if interval <= 0 || burst <= 0 {
        if interval < 0 || burst < 0 {
            rs.warn_negative(interval, burst);
        }
        ret = c_int::from(interval == 0 || burst > 0);
        if rs.flags() & RATELIMIT_INITIALIZED == 0
            || (interval == 0 && burst == 0)
            || !rs.trylock_irqsave(&mut flags)
        {
            return nolock_ret(rs, ret);
        }

        /* Force re-initialization once re-enabled. */
        rs.set_flags(rs.flags() & !RATELIMIT_INITIALIZED);
        return unlock_ret(rs, flags, ret);
    }

    /*
     * If we contend on this state's lock then just check if
     * the current burst is used or not. It might cause
     * false positive when we are past the interval and
     * the current lock owner is just about to reset it.
     */
    if !rs.trylock_irqsave(&mut flags) {
        if rs.flags() & RATELIMIT_INITIALIZED != 0 && rs.n_left() > 0 && rs.dec_return_n_left() >= 0
        {
            ret = 1;
        }
        return nolock_ret(rs, ret);
    }

    if rs.flags() & RATELIMIT_INITIALIZED == 0 {
        rs.set_begin(rs.jiffies());
        rs.set_flags(rs.flags() | RATELIMIT_INITIALIZED);
        rs.set_n_left(rs.burst());
    }

    // `rs->begin + interval`: an unsigned long plus an int, which C converts
    // to unsigned long. It is read before jiffies, as in the C macro.
    let end = rs.begin().wrapping_add(interval as usize);
    if time_is_before_jiffies(rs, end) {
        /*
         * Reset rs_n_left ASAP to reduce false positives
         * in parallel calls, see above.
         */
        rs.set_n_left(rs.burst());
        rs.set_begin(rs.jiffies());

        if rs.flags() & RATELIMIT_MSG_ON_RELEASE == 0 {
            let m = rs.reset_miss();
            if m != 0 {
                rs.printk_suppressed(m);
            }
        }
    }

    /* Note that the burst might be taken by a parallel call. */
    if rs.n_left() > 0 && rs.dec_return_n_left() >= 0 {
        ret = 1;
    }

    unlock_ret(rs, flags, ret)
}

/// `__ratelimit` - rate limiting.
///
/// This enforces a rate limit: not more than `rs->burst` callbacks in every
/// `rs->interval`.
///
/// Returns 0 when callbacks will be suppressed, 1 when to go ahead and do it.
///
/// # Safety
///
/// `rs` is a live, initialized C `struct ratelimit_state`, and `func` is a
/// NUL-terminated string that lives for the call, as `__ratelimit()` passes
/// `__func__`. C code may access the state concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ___ratelimit(rs: *mut ratelimit_state, func: *const c_char) -> c_int {
    // SAFETY: (U3) the C contract of ___ratelimit(): rs is the caller's live
    // ratelimit_state. Every field is interior-mutable, so a shared reference
    // is sound while C accesses the state.
    let state = unsafe { &*rs };

    ratelimit(&Live { rs, state, func })
}
