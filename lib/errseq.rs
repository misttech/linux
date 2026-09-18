// SPDX-License-Identifier: GPL-2.0

//! An errseq_t is a way of recording errors in one place, and allowing any
//! number of "subscribers" to tell whether it has changed since a previous
//! point where it was sampled.
//!
//! It's implemented as an unsigned 32-bit value. The low order bits are
//! designated to hold an error code (between 0 and -MAX_ERRNO). The upper bits
//! are used as a counter. This is done with atomics instead of locking so that
//! these functions can be called from any context.
//!
//! The general idea is for consumers to sample an errseq_t value. That value
//! can later be used to tell whether any new errors have occurred since that
//! sampling was done.
//!
//! Note that there is a risk of collisions if new errors are being recorded
//! frequently, since we have so few bits to use as a counter.
//!
//! To mitigate this, one bit is used as a flag to tell whether the value has
//! been sampled since a new value was recorded. That allows us to avoid bumping
//! the counter if no one has sampled it since the last time an error was
//! recorded.
//!
//! A new errseq_t should always be zeroed out.  A errseq_t value of all zeroes
//! is the special (but common) case where there has never been an error. An all
//! zero value thus serves as the "epoch" if one wishes to know whether there
//! has ever been an error set since it was first initialized.
//!
//! The Rust implementation of `lib/errseq.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/errseq.h` is the ABI contract.
//! `lib/errseq_ffi.c` exports these symbols with the license of `lib/errseq.c`,
//! and wraps the `WARN()` on an out-of-range error.

use core::ffi::c_int;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

/// Mirror of `errseq_t` (`include/linux/errseq.h`): a typedef of `u32`.
#[allow(non_camel_case_types)]
pub type errseq_t = u32;

// Typedef of u32: no pointer, no lock, no config. Values from
// `pahole -C errseq_t /sys/kernel/btf/vmlinux` (typedef u32) and a userspace
// compile of the same typedef (size 4, align 4).
kr::static_assert_layout!(errseq_t, size = 4, align = 4);

/// `MAX_ERRNO` from `include/linux/err.h`.
const MAX_ERRNO: u32 = 4095;

/// `ERRSEQ_SHIFT`: `ilog2(MAX_ERRNO) + 1`.
const ERRSEQ_SHIFT: u32 = MAX_ERRNO.ilog2() + 1;

/// This bit is used as a flag to indicate whether the value has been seen.
pub(crate) const ERRSEQ_SEEN: errseq_t = 1 << ERRSEQ_SHIFT;

/// Leverage `ERRSEQ_SEEN` to define the errno mask here.
pub(crate) const ERRNO_MASK: errseq_t = ERRSEQ_SEEN - 1;

/// The lowest bit of the counter.
const ERRSEQ_CTR_INC: errseq_t = 1 << (ERRSEQ_SHIFT + 1);

kr::static_assert!(ERRSEQ_SHIFT == 12);
kr::static_assert!(ERRSEQ_SEEN == 0x1000);
kr::static_assert!(ERRNO_MASK == 0x0fff);
kr::static_assert!(ERRSEQ_CTR_INC == 0x2000);

unsafe extern "C" {
    fn c_errseq_warn_bad_err(err: c_int);
}

/// The atomic operations on an `errseq_t` that the functions below use.
///
/// Implemented here for `AtomicU32`. The harness implements it for loom's
/// atomic, so its model runs the code in this file.
pub(crate) trait ErrseqAtomic {
    /// `READ_ONCE()`.
    fn read_once(&self) -> errseq_t;

    /// `cmpxchg()`: returns the previous value.
    fn cmpxchg(&self, old: errseq_t, new: errseq_t) -> errseq_t;
}

// The orderings follow the kernel's: READ_ONCE() carries no LKMM ordering
// (Relaxed), and cmpxchg() is fully ordered (SeqCst).
impl ErrseqAtomic for AtomicU32 {
    #[inline]
    fn read_once(&self) -> errseq_t {
        self.load(Ordering::Relaxed)
    }

    #[inline]
    fn cmpxchg(&self, old: errseq_t, new: errseq_t) -> errseq_t {
        match self.compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(v) => v,
            Err(v) => v,
        }
    }
}

/// True when `err` is 0 or outside `-1..-MAX_ERRNO`, the `WARN()` condition in
/// `errseq_set()`.
fn err_out_of_range(err: c_int) -> bool {
    err == 0 || (err.wrapping_neg() as u32) > MAX_ERRNO
}

/// The body of `errseq_set()`, generic over the atomic.
///
/// `warn_bad` is the `WARN()` on an out-of-range error.
pub(crate) fn set<A: ErrseqAtomic>(eseq: &A, err: c_int, warn_bad: impl FnOnce(c_int)) -> errseq_t {
    /*
     * Ensure the error code actually fits where we want it to go. If it
     * doesn't then just throw a warning and don't record anything. We
     * also don't accept zero here as that would effectively clear a
     * previous error.
     */
    let mut old = eseq.read_once();

    if err_out_of_range(err) {
        warn_bad(err);
        return old;
    }

    loop {
        /* Clear out error bits and set new error */
        let mut new = (old & !(ERRNO_MASK | ERRSEQ_SEEN)) | (err.wrapping_neg() as errseq_t);

        /* Only increment if someone has looked at it */
        if old & ERRSEQ_SEEN != 0 {
            new = new.wrapping_add(ERRSEQ_CTR_INC);
        }

        /* If there would be no change, then call it done */
        if new == old {
            return new;
        }

        /* Try to swap the new value into place */
        let cur = eseq.cmpxchg(old, new);

        /*
         * Call it success if we did the swap or someone else beat us
         * to it for the same value.
         */
        if cur == old || cur == new {
            return cur;
        }

        /* Raced with an update, try again */
        old = cur;
    }
}

/// The body of `errseq_sample()`, generic over the atomic.
pub(crate) fn sample<A: ErrseqAtomic>(eseq: &A) -> errseq_t {
    let old = eseq.read_once();

    /* If nobody has seen this error yet, then we can be the first. */
    if old & ERRSEQ_SEEN == 0 {
        0
    } else {
        old
    }
}

/// The body of `errseq_check()`, generic over the atomic.
pub(crate) fn check<A: ErrseqAtomic>(eseq: &A, since: errseq_t) -> c_int {
    let cur = eseq.read_once();

    if cur == since {
        0
    } else {
        -((cur & ERRNO_MASK) as c_int)
    }
}

/// The body of `errseq_check_and_advance()`, generic over the atomic.
///
/// The caller serializes updates of `since`, as the C comment requires.
pub(crate) fn check_and_advance<A: ErrseqAtomic>(eseq: &A, since: &mut errseq_t) -> c_int {
    let mut err = 0;
    /*
     * Most callers will want to use the inline wrapper to check this,
     * so that the common case of no error is handled without needing
     * to take the lock that protects the "since" value.
     */
    let old = eseq.read_once();
    if old != *since {
        /*
         * Set the flag and try to swap it into place if it has
         * changed.
         *
         * We don't care about the outcome of the swap here. If the
         * swap doesn't occur, then it has either been updated by a
         * writer who is altering the value in some way (updating
         * counter or resetting the error), or another reader who is
         * just setting the "seen" flag. Either outcome is OK, and we
         * can advance "since" and return an error based on what we
         * have.
         */
        let new = old | ERRSEQ_SEEN;
        if new != old {
            let _ = eseq.cmpxchg(old, new);
        }
        *since = new;
        err = -((new & ERRNO_MASK) as c_int);
    }
    err
}

/// Set a errseq_t for later reporting.
///
/// This function sets the error in `eseq`, and increments the sequence counter
/// if the last sequence was sampled at some point in the past.
///
/// Any error set will always overwrite an existing error.
///
/// Return: The previous value, primarily for debugging purposes. The
/// return value should not be used as a previously sampled value in later
/// calls as it will not have the SEEN flag set.
///
/// # Safety
///
/// `eseq` points to a live `errseq_t` for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn errseq_set(eseq: *mut errseq_t, err: c_int) -> errseq_t {
    // SAFETY: (U3) the C contract of errseq_set(): eseq is the caller's
    // errseq_t, live for this call. Atomic ops on a plain u32 match the
    // C READ_ONCE / cmpxchg.
    let eseq = unsafe { AtomicU32::from_ptr(eseq) };
    set(eseq, err, |e| {
        // SAFETY: (U1) c_errseq_warn_bad_err() takes the rejected errno and
        // only WARN()s.
        unsafe { c_errseq_warn_bad_err(e) }
    })
}

/// Grab current errseq_t value.
///
/// This function allows callers to initialise their errseq_t variable.
/// If the error has been "seen", new callers will not see an old error.
/// If there is an unseen error in `eseq`, the caller of this function will
/// see it the next time it checks for an error.
///
/// Context: Any context.
/// Return: The current errseq value.
///
/// # Safety
///
/// As [`errseq_set()`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn errseq_sample(eseq: *mut errseq_t) -> errseq_t {
    // SAFETY: (U3) the C contract of errseq_sample(): eseq is the caller's
    // errseq_t, live for this call.
    let eseq = unsafe { AtomicU32::from_ptr(eseq) };
    sample(eseq)
}

/// Has an error occurred since a particular sample point?
///
/// Grab the value that eseq points to, and see if it has changed `since`
/// the given value was sampled. The `since` value is not advanced, so there
/// is no need to mark the value as seen.
///
/// Return: The latest error set in the errseq_t or 0 if it hasn't changed.
///
/// # Safety
///
/// As [`errseq_set()`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn errseq_check(eseq: *mut errseq_t, since: errseq_t) -> c_int {
    // SAFETY: (U3) the C contract of errseq_check(): eseq is the caller's
    // errseq_t, live for this call.
    let eseq = unsafe { AtomicU32::from_ptr(eseq) };
    check(eseq, since)
}

/// Check an errseq_t and advance to current value.
///
/// Grab the eseq value, and see whether it matches the value that `since`
/// points to. If it does, then just return 0.
///
/// If it doesn't, then the value has changed. Set the "seen" flag, and try to
/// swap it into place as the new eseq value. Then, set that value as the new
/// "since" value, and return whatever the error portion is set to.
///
/// Note that no locking is provided here for concurrent updates to the "since"
/// value. The caller must provide that if necessary. Because of this, callers
/// may want to do a lockless errseq_check before taking the lock and calling
/// this.
///
/// Return: Negative errno if one has been stored, or 0 if no new error has
/// occurred.
///
/// # Safety
///
/// `eseq` and `since` point to live `errseq_t` values for the duration of the
/// call. The caller serializes concurrent updates of `since`. The C header
/// does not require the two pointers to be distinct. Distinct pointers become
/// `&AtomicU32` and `&mut`; the aliased case uses only raw access, so the two
/// references are never formed to the same word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn errseq_check_and_advance(
    eseq: *mut errseq_t,
    since: *mut errseq_t,
) -> c_int {
    if !ptr::eq(eseq, since) {
        // SAFETY: (U3) the pointers are distinct and both live, so the
        // &AtomicU32 and &mut errseq_t do not overlap.
        let eseq = unsafe { AtomicU32::from_ptr(eseq) };
        // SAFETY: (U3) since is live and distinct from eseq.
        let since = unsafe { &mut *since };
        return check_and_advance(eseq, since);
    }

    let old = {
        // SAFETY: (U3) eseq is live. The &AtomicU32 does not outlive this
        // block, so it cannot overlap the later write through since.
        unsafe { AtomicU32::from_ptr(eseq) }.read_once()
    };
    // SAFETY: (U3) since aliases eseq and is live.
    let since_val = unsafe { ptr::read(since) };
    if old == since_val {
        return 0;
    }
    let new = old | ERRSEQ_SEEN;
    if new != old {
        // SAFETY: (U3) as the load above. Dropped before the write through
        // since.
        let _ = unsafe { AtomicU32::from_ptr(eseq) }.cmpxchg(old, new);
    }
    // SAFETY: (U3) since aliases eseq. No &AtomicU32 is held.
    unsafe { ptr::write(since, new) };
    -((new & ERRNO_MASK) as c_int)
}
