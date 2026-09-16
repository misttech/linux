// SPDX-License-Identifier: GPL-2.0

//! Decrement a reference count, and return locked if it decremented to zero.
//!
//! The Rust implementation of `lib/dec_and_lock.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/spinlock.h` declares these functions
//! and is the ABI contract. `lib/dec_and_lock_ffi.c` exports them with the
//! license of `lib/dec_and_lock.c`, and wraps the locking macros called here.
//!
//! This is an implementation of the notion of "decrement a
//! reference count, and return locked if it decremented to zero".
//!
//! NOTE NOTE NOTE! This is _not_ equivalent to
//!
//! ```text
//!        if (atomic_dec_and_test(&atomic)) {
//!                spin_lock(&lock);
//!                return 1;
//!        }
//!        return 0;
//! ```
//!
//! because the spin-lock and the decrement must be
//! "atomic".

use crate::refcount::{atomic_t, spinlock_t};
use core::ffi::{c_int, c_ulong, c_void};
use core::sync::atomic::{AtomicI32, Ordering};

/// `raw_spinlock_t`. Never dereferenced in Rust, only passed to C.
#[allow(non_camel_case_types)]
pub type raw_spinlock_t = c_void;

extern "C" {
    fn c_dec_and_lock_spin_lock(lock: *mut spinlock_t);
    fn c_dec_and_lock_spin_unlock(lock: *mut spinlock_t);
    fn c_dec_and_lock_spin_lock_irqsave(lock: *mut spinlock_t, flags: *mut c_ulong);
    fn c_dec_and_lock_spin_unlock_irqrestore(lock: *mut spinlock_t, flags: *mut c_ulong);

    fn c_dec_and_lock_raw_spin_lock(lock: *mut raw_spinlock_t);
    fn c_dec_and_lock_raw_spin_unlock(lock: *mut raw_spinlock_t);
    fn c_dec_and_lock_raw_spin_lock_irqsave(lock: *mut raw_spinlock_t, flags: *mut c_ulong);
    fn c_dec_and_lock_raw_spin_unlock_irqrestore(lock: *mut raw_spinlock_t, flags: *mut c_ulong);
}

/// The fully ordered atomic operations this unit uses.
///
/// `lib/refcount.rs`'s `RefsAtomic` carries the relaxed and release forms the
/// refcount family needs. `atomic_add_unless()` and `atomic_dec_and_test()`
/// are fully ordered, so they get their own vocabulary here rather than
/// widening a trait that two loom models already implement.
///
/// The harness implements it for loom's atomic, so its model runs the code in
/// this file.
pub(crate) trait DecAtomic {
    /// `atomic_read()`.
    fn read(&self) -> i32;

    /// `atomic_try_cmpxchg()`: fully ordered. On failure, `old` gets the
    /// current value.
    fn try_cmpxchg(&self, old: &mut i32, new: i32) -> bool;

    /// `atomic_fetch_sub()`: fully ordered, returning the old value.
    fn fetch_sub(&self, i: i32) -> i32;
}

// atomic_add_unless() and atomic_dec_and_test() are fully ordered in LKMM,
// which is SeqCst here. The failed compare-exchange load stays Relaxed,
// as the successful ordering defines it.
impl DecAtomic for AtomicI32 {
    #[inline]
    fn read(&self) -> i32 {
        self.load(Ordering::SeqCst)
    }

    #[inline]
    fn try_cmpxchg(&self, old: &mut i32, new: i32) -> bool {
        match self.compare_exchange(*old, new, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(_) => true,
            Err(current) => {
                *old = current;
                false
            }
        }
    }

    #[inline]
    fn fetch_sub(&self, i: i32) -> i32 {
        self.fetch_sub(i, Ordering::SeqCst)
    }
}

/// The body of `raw_atomic_fetch_add_unless()` in
/// `include/linux/atomic/atomic-arch-fallback.h`, generic over the atomic.
///
/// Returns the old value.
pub(crate) fn fetch_add_unless(v: &impl DecAtomic, a: i32, u: i32) -> i32 {
    let mut c = v.read();

    loop {
        if c == u {
            break;
        }
        let new = c.wrapping_add(a);
        if v.try_cmpxchg(&mut c, new) {
            break;
        }
    }

    c
}

/// `atomic_add_unless()`: adds `a` unless the value is `u`.
///
/// Returns true if the value was updated.
pub(crate) fn add_unless(v: &impl DecAtomic, a: i32, u: i32) -> bool {
    fetch_add_unless(v, a, u) != u
}

/// `atomic_dec_and_test()`: decrements and returns whether the result is zero.
pub(crate) fn dec_and_test(v: &impl DecAtomic) -> bool {
    v.fetch_sub(1) == 1
}

/// `atomic_dec_and_lock()`: lock on reaching reference count zero.
///
/// Decrements `atomic` by 1. If the result is 0, returns true and locks
/// `lock`. Returns false for all other cases.
///
/// # Safety
///
/// `atomic` points to a live `atomic_t` on which the caller holds a
/// reference, and `lock` to an initialized spinlock.
#[no_mangle]
pub unsafe extern "C" fn atomic_dec_and_lock(
    atomic: *mut atomic_t,
    lock: *mut spinlock_t,
) -> c_int {
    // SAFETY: (U3) the caller passes a live atomic_t, as the prototype in
    // include/linux/spinlock.h requires.
    let v = unsafe { &*atomic };

    // Subtract 1 from counter unless that drops it to 0 (ie. it was 1)
    if add_unless(v.counter(), -1, 1) {
        return 0;
    }

    // Otherwise do it the slow way
    // SAFETY: (U1) spin_lock(): lock is an initialized spinlock.
    unsafe { c_dec_and_lock_spin_lock(lock) };
    if dec_and_test(v.counter()) {
        return 1;
    }
    // SAFETY: (U1) spin_unlock(): the lock was taken just above.
    unsafe { c_dec_and_lock_spin_unlock(lock) };
    0
}

/// `_atomic_dec_and_lock_irqsave()`: as [`atomic_dec_and_lock()`], with the
/// spinlock taken with interrupts disabled.
///
/// # Safety
///
/// `atomic` points to a live `atomic_t` on which the caller holds a
/// reference, `lock` to an initialized spinlock, and `flags` to the caller's
/// saved IRQ flags.
#[no_mangle]
pub unsafe extern "C" fn _atomic_dec_and_lock_irqsave(
    atomic: *mut atomic_t,
    lock: *mut spinlock_t,
    flags: *mut c_ulong,
) -> c_int {
    // SAFETY: (U3) the caller passes a live atomic_t, as the prototype in
    // include/linux/spinlock.h requires.
    let v = unsafe { &*atomic };

    // Subtract 1 from counter unless that drops it to 0 (ie. it was 1)
    if add_unless(v.counter(), -1, 1) {
        return 0;
    }

    // Otherwise do it the slow way
    // SAFETY: (U1) spin_lock_irqsave(): lock is an initialized spinlock, and
    // flags points to the caller's unsigned long.
    unsafe { c_dec_and_lock_spin_lock_irqsave(lock, flags) };
    if dec_and_test(v.counter()) {
        return 1;
    }
    // SAFETY: (U1) spin_unlock_irqrestore(): the lock was taken just above,
    // and flags holds what spin_lock_irqsave() saved.
    unsafe { c_dec_and_lock_spin_unlock_irqrestore(lock, flags) };
    0
}

/// `atomic_dec_and_raw_lock()`: as [`atomic_dec_and_lock()`], on a
/// `raw_spinlock_t`.
///
/// # Safety
///
/// `atomic` points to a live `atomic_t` on which the caller holds a
/// reference, and `lock` to an initialized raw spinlock.
#[no_mangle]
pub unsafe extern "C" fn atomic_dec_and_raw_lock(
    atomic: *mut atomic_t,
    lock: *mut raw_spinlock_t,
) -> c_int {
    // SAFETY: (U3) the caller passes a live atomic_t, as the prototype in
    // include/linux/spinlock.h requires.
    let v = unsafe { &*atomic };

    // Subtract 1 from counter unless that drops it to 0 (ie. it was 1)
    if add_unless(v.counter(), -1, 1) {
        return 0;
    }

    // Otherwise do it the slow way
    // SAFETY: (U1) raw_spin_lock(): lock is an initialized raw spinlock.
    unsafe { c_dec_and_lock_raw_spin_lock(lock) };
    if dec_and_test(v.counter()) {
        return 1;
    }
    // SAFETY: (U1) raw_spin_unlock(): the lock was taken just above.
    unsafe { c_dec_and_lock_raw_spin_unlock(lock) };
    0
}

/// `_atomic_dec_and_raw_lock_irqsave()`: as [`atomic_dec_and_raw_lock()`],
/// with the raw spinlock taken with interrupts disabled.
///
/// # Safety
///
/// `atomic` points to a live `atomic_t` on which the caller holds a
/// reference, `lock` to an initialized raw spinlock, and `flags` to the
/// caller's saved IRQ flags.
#[no_mangle]
pub unsafe extern "C" fn _atomic_dec_and_raw_lock_irqsave(
    atomic: *mut atomic_t,
    lock: *mut raw_spinlock_t,
    flags: *mut c_ulong,
) -> c_int {
    // SAFETY: (U3) the caller passes a live atomic_t, as the prototype in
    // include/linux/spinlock.h requires.
    let v = unsafe { &*atomic };

    // Subtract 1 from counter unless that drops it to 0 (ie. it was 1)
    if add_unless(v.counter(), -1, 1) {
        return 0;
    }

    // Otherwise do it the slow way
    // SAFETY: (U1) raw_spin_lock_irqsave(): lock is an initialized raw
    // spinlock, and flags points to the caller's unsigned long.
    unsafe { c_dec_and_lock_raw_spin_lock_irqsave(lock, flags) };
    if dec_and_test(v.counter()) {
        return 1;
    }
    // SAFETY: (U1) raw_spin_unlock_irqrestore(): the lock was taken just
    // above, and flags holds what raw_spin_lock_irqsave() saved.
    unsafe { c_dec_and_lock_raw_spin_unlock_irqrestore(lock, flags) };
    0
}
