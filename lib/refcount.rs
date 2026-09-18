// SPDX-License-Identifier: GPL-2.0

//! Out-of-line refcount functions.
//!
//! The Rust implementation of `lib/refcount.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/refcount.h` is the ABI contract, and
//! its static inlines stay in C. `lib/refcount_ffi.c` exports these symbols
//! with the license of `lib/refcount.c`, and wraps the C macros and inlines
//! called from here.
//!
//! Rust callers use the typed API on [`refcount_t`], which implements the
//! operations of the header's inlines in Rust.

use core::ffi::{c_uint, c_ulong, c_void};
use core::ptr;
use core::sync::atomic::{AtomicI32, Ordering};

/// Mirror of `atomic_t` (`include/linux/types.h`).
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct atomic_t {
    counter: AtomicI32,
}

kr::static_assert_layout!(atomic_t, size = 4, align = 4, counter @ 0);

impl atomic_t {
    /// `ATOMIC_INIT(i)`.
    pub const fn new(i: i32) -> Self {
        Self {
            counter: AtomicI32::new(i),
        }
    }

    /// The counter, on which [`RefsAtomic`] implements the `<linux/atomic.h>`
    /// operations that this unit and the other ports use.
    pub(crate) fn counter(&self) -> &AtomicI32 {
        &self.counter
    }
}

/// Mirror of `refcount_t`, which is `struct refcount_struct` in
/// `include/linux/refcount_types.h`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct refcount_t {
    refs: atomic_t,
}

kr::static_assert_layout!(refcount_t, size = 4, align = 4, refs @ 0);

/// `struct mutex`. Never dereferenced in Rust, only passed to C.
#[allow(non_camel_case_types)]
pub type mutex = c_void;

/// `spinlock_t`. Never dereferenced in Rust, only passed to C.
#[allow(non_camel_case_types)]
pub type spinlock_t = c_void;

/// `enum refcount_saturation_type`.
///
/// An integer rather than a Rust enum: C may pass any value, and
/// [`refcount_warn_saturate()`] has a default case for the others.
#[allow(non_camel_case_types)]
pub type refcount_saturation_type = c_uint;

/// `REFCOUNT_ADD_NOT_ZERO_OVF`.
pub const REFCOUNT_ADD_NOT_ZERO_OVF: refcount_saturation_type = 0;
/// `REFCOUNT_ADD_OVF`.
pub const REFCOUNT_ADD_OVF: refcount_saturation_type = 1;
/// `REFCOUNT_ADD_UAF`.
pub const REFCOUNT_ADD_UAF: refcount_saturation_type = 2;
/// `REFCOUNT_SUB_UAF`.
pub const REFCOUNT_SUB_UAF: refcount_saturation_type = 3;
/// `REFCOUNT_DEC_LEAK`.
pub const REFCOUNT_DEC_LEAK: refcount_saturation_type = 4;

/// `REFCOUNT_SATURATED`.
pub const REFCOUNT_SATURATED: i32 = i32::MIN / 2;

unsafe extern "C" {
    // One wrapper per WARN_ONCE() site in lib/refcount.c, so that each keeps
    // its own once flag.
    fn c_refcount_warn_add_not_zero_ovf();
    fn c_refcount_warn_add_ovf();
    fn c_refcount_warn_add_uaf();
    fn c_refcount_warn_sub_uaf();
    fn c_refcount_warn_dec_leak();
    fn c_refcount_warn_unknown();
    fn c_refcount_warn_dec_not_one_underflow();

    fn c_refcount_acquire_after_ctrl_dep();

    fn c_refcount_dec_and_test(r: *mut refcount_t) -> bool;
    fn c_refcount_mutex_lock(lock: *mut mutex);
    fn c_refcount_mutex_unlock(lock: *mut mutex);
    fn c_refcount_spin_lock(lock: *mut spinlock_t);
    fn c_refcount_spin_unlock(lock: *mut spinlock_t);
    fn c_refcount_spin_lock_irqsave(lock: *mut spinlock_t, flags: *mut c_ulong);
    fn c_refcount_spin_unlock_irqrestore(lock: *mut spinlock_t, flags: *mut c_ulong);
}

/// The atomic operations on the counter that the functions below use.
///
/// Implemented here for `AtomicI32`. The harness implements it for loom's
/// atomic, so its model runs the code in this file.
pub(crate) trait RefsAtomic {
    /// `atomic_read()`.
    fn read(&self) -> i32;

    /// `atomic_set()`.
    fn set(&self, i: i32);

    /// `atomic_fetch_add_relaxed()`.
    fn fetch_add_relaxed(&self, i: i32) -> i32;

    /// `atomic_fetch_sub_release()`.
    fn fetch_sub_release(&self, i: i32) -> i32;

    /// `atomic_try_cmpxchg_relaxed()`: on failure, `old` gets the current value.
    fn try_cmpxchg_relaxed(&self, old: &mut i32, new: i32) -> bool;

    /// `atomic_try_cmpxchg_release()`: on failure, `old` gets the current value.
    fn try_cmpxchg_release(&self, old: &mut i32, new: i32) -> bool;
}

// The orderings follow the kernel's: atomic_read() and atomic_set() carry no
// LKMM annotation and are fully ordered (SeqCst), _relaxed is Relaxed, and a
// _release store is Release. The load of a failed compare-exchange stays
// Relaxed, as Release gives it.
impl RefsAtomic for AtomicI32 {
    #[inline]
    fn read(&self) -> i32 {
        self.load(Ordering::SeqCst)
    }

    #[inline]
    fn set(&self, i: i32) {
        self.store(i, Ordering::SeqCst);
    }

    #[inline]
    fn fetch_add_relaxed(&self, i: i32) -> i32 {
        self.fetch_add(i, Ordering::Relaxed)
    }

    #[inline]
    fn fetch_sub_release(&self, i: i32) -> i32 {
        self.fetch_sub(i, Ordering::Release)
    }

    #[inline]
    fn try_cmpxchg_relaxed(&self, old: &mut i32, new: i32) -> bool {
        match self.compare_exchange(*old, new, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => true,
            Err(current) => {
                *old = current;
                false
            }
        }
    }

    #[inline]
    fn try_cmpxchg_release(&self, old: &mut i32, new: i32) -> bool {
        match self.compare_exchange(*old, new, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => true,
            Err(current) => {
                *old = current;
                false
            }
        }
    }
}

impl refcount_t {
    /// `REFCOUNT_INIT(n)`.
    pub const fn new(n: i32) -> Self {
        Self {
            refs: atomic_t::new(n),
        }
    }

    /// `refcount_set()`: sets the refcount's value.
    #[inline]
    pub fn set(&self, n: i32) {
        self.refs.counter.set(n);
    }

    /// `refcount_read()`: returns the refcount's value.
    #[inline]
    pub fn read(&self) -> u32 {
        self.refs.counter.read() as u32
    }

    /// `refcount_inc()`: increments the refcount.
    ///
    /// Similar to `atomic_inc()`, but will saturate at `REFCOUNT_SATURATED` and
    /// WARN.
    ///
    /// Provides no memory ordering, it is assumed the caller already has a
    /// reference on the object.
    ///
    /// Will WARN if the refcount is 0, as this represents a possible
    /// use-after-free condition.
    #[inline]
    pub fn inc(&self) {
        add(&self.refs.counter, 1, |t| self.warn_saturate(t));
    }

    /// `refcount_inc_not_zero()`: increments the refcount unless it is 0.
    ///
    /// Similar to `atomic_inc_not_zero()`, but will saturate at
    /// `REFCOUNT_SATURATED` and WARN.
    ///
    /// Provides no memory ordering, it is assumed the caller has guaranteed the
    /// object memory to be stable (RCU, etc.). It does provide a control
    /// dependency and thereby orders future stores. See the comment on top of
    /// `include/linux/refcount.h`.
    ///
    /// Returns true if the increment was successful, false otherwise.
    #[inline]
    #[must_use]
    pub fn inc_not_zero(&self) -> bool {
        add_not_zero(&self.refs.counter, 1, |t| self.warn_saturate(t)) != 0
    }

    /// `refcount_dec_and_test()`: decrements the refcount and tests if it is 0.
    ///
    /// Similar to `atomic_dec_and_test()`, it will WARN on underflow and fail to
    /// decrement when saturated at `REFCOUNT_SATURATED`.
    ///
    /// Provides release memory ordering, such that prior loads and stores are
    /// done before, and provides an acquire ordering on success such that
    /// free() must come after.
    ///
    /// Returns true if the resulting refcount is 0, false otherwise.
    #[inline]
    #[must_use]
    pub fn dec_and_test(&self) -> bool {
        sub_and_test(
            &self.refs.counter,
            1,
            || {
                // SAFETY: (U1) c_refcount_acquire_after_ctrl_dep() takes no
                // arguments and only issues smp_acquire__after_ctrl_dep().
                unsafe { c_refcount_acquire_after_ctrl_dep() }
            },
            |t| self.warn_saturate(t),
        )
        .0
    }

    #[cold]
    fn warn_saturate(&self, t: refcount_saturation_type) {
        // SAFETY: (U3) the pointer comes from &self, so it points to a live
        // refcount_t.
        unsafe { refcount_warn_saturate(ptr::from_ref(self).cast_mut(), t) }
    }
}

/// The body of `__refcount_add()` in `include/linux/refcount.h`, generic over
/// the atomic.
///
/// Returns the old value. `warn` is `refcount_warn_saturate()`.
pub(crate) fn add(
    refs: &impl RefsAtomic,
    i: i32,
    warn: impl FnOnce(refcount_saturation_type),
) -> i32 {
    let old = refs.fetch_add_relaxed(i);

    if old == 0 {
        warn(REFCOUNT_ADD_UAF);
    } else if old < 0 || old.wrapping_add(i) < 0 {
        warn(REFCOUNT_ADD_OVF);
    }

    old
}

/// The body of `__refcount_add_not_zero()` in `include/linux/refcount.h`,
/// generic over the atomic.
///
/// Returns the old value, which is 0 when nothing was added. `warn` is
/// `refcount_warn_saturate()`.
pub(crate) fn add_not_zero(
    refs: &impl RefsAtomic,
    i: i32,
    warn: impl FnOnce(refcount_saturation_type),
) -> i32 {
    let mut old = refs.read();

    loop {
        if old == 0 {
            break;
        }
        let new = old.wrapping_add(i);
        if refs.try_cmpxchg_relaxed(&mut old, new) {
            break;
        }
    }

    if old < 0 || old.wrapping_add(i) < 0 {
        warn(REFCOUNT_ADD_NOT_ZERO_OVF);
    }

    old
}

/// The body of `__refcount_sub_and_test()` in `include/linux/refcount.h`,
/// generic over the atomic.
///
/// Returns whether the count reached 0, and the old value.
/// `acquire_after_ctrl_dep` is `smp_acquire__after_ctrl_dep()`, and `warn` is
/// `refcount_warn_saturate()`.
pub(crate) fn sub_and_test(
    refs: &impl RefsAtomic,
    i: i32,
    acquire_after_ctrl_dep: impl FnOnce(),
    warn: impl FnOnce(refcount_saturation_type),
) -> (bool, i32) {
    let old = refs.fetch_sub_release(i);

    if old > 0 && old == i {
        acquire_after_ctrl_dep();
        return (true, old);
    }

    if old <= 0 || old.wrapping_sub(i) < 0 {
        warn(REFCOUNT_SUB_UAF);
    }

    (false, old)
}

/// `refcount_warn_saturate()`: saturates `r` and warns about event `t`.
///
/// # Safety
///
/// `r` points to a `refcount_t` that the calling operation in
/// `include/linux/refcount.h` has just read or written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn refcount_warn_saturate(r: *mut refcount_t, t: refcount_saturation_type) {
    // SAFETY: (U3) every caller in include/linux/refcount.h has just accessed
    // *r through the same pointer, so it points to a refcount_t.
    let r = unsafe { &*r };

    // refcount_set(r, REFCOUNT_SATURATED)
    r.refs.counter.store(REFCOUNT_SATURATED, Ordering::SeqCst);

    // SAFETY: (U1) the c_refcount_warn_*() wrappers take no arguments and only
    // WARN_ONCE().
    unsafe {
        match t {
            REFCOUNT_ADD_NOT_ZERO_OVF => c_refcount_warn_add_not_zero_ovf(),
            REFCOUNT_ADD_OVF => c_refcount_warn_add_ovf(),
            REFCOUNT_ADD_UAF => c_refcount_warn_add_uaf(),
            REFCOUNT_SUB_UAF => c_refcount_warn_sub_uaf(),
            REFCOUNT_DEC_LEAK => c_refcount_warn_dec_leak(),
            _ => c_refcount_warn_unknown(),
        }
    }
}

/// `refcount_dec_if_one()`: decrements a refcount if it is 1.
///
/// No `atomic_t` counterpart, it attempts a 1 -> 0 transition and returns the
/// success thereof.
///
/// Like all decrement operations, it provides release memory order and
/// provides a control dependency.
///
/// It can be used like a try-delete operator; this explicit case is provided
/// and not cmpxchg in generic, because that would allow implementing unsafe
/// operations.
///
/// Returns true if the resulting refcount is 0, false otherwise.
///
/// # Safety
///
/// `r` points to a `refcount_t` on which the caller holds a reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn refcount_dec_if_one(r: *mut refcount_t) -> bool {
    // SAFETY: (U4) the caller holds a reference, so r points to a live
    // refcount_t.
    let r = unsafe { &*r };

    dec_if_one(&r.refs.counter)
}

/// The body of [`refcount_dec_if_one()`], generic over the atomic.
pub(crate) fn dec_if_one(refs: &impl RefsAtomic) -> bool {
    let mut val = 1;

    refs.try_cmpxchg_release(&mut val, 0)
}

/// `refcount_dec_not_one()`: decrements a refcount if it is not 1.
///
/// No `atomic_t` counterpart, it decrements unless the value is 1, in which
/// case it will return false.
///
/// Was often done like: `atomic_add_unless(&var, -1, 1)`
///
/// Returns true if the decrement operation was successful, false otherwise.
///
/// # Safety
///
/// `r` points to a `refcount_t` on which the caller holds a reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn refcount_dec_not_one(r: *mut refcount_t) -> bool {
    // SAFETY: (U4) the caller holds a reference, so r points to a live
    // refcount_t.
    let r = unsafe { &*r };

    dec_not_one(&r.refs.counter, || {
        // SAFETY: (U1) c_refcount_warn_dec_not_one_underflow() takes no
        // arguments and only WARN_ONCE()s.
        unsafe { c_refcount_warn_dec_not_one_underflow() }
    })
}

/// The body of [`refcount_dec_not_one()`], generic over the atomic.
///
/// `warn_underflow` is the WARN_ONCE() on underflow.
pub(crate) fn dec_not_one(refs: &impl RefsAtomic, warn_underflow: impl FnOnce()) -> bool {
    let mut val = refs.read() as u32;

    loop {
        if val == REFCOUNT_SATURATED as u32 {
            return true;
        }

        if val == 1 {
            return false;
        }

        let new = val.wrapping_sub(1);
        if new > val {
            warn_underflow();
            return true;
        }

        let mut old = val as i32;
        if refs.try_cmpxchg_release(&mut old, new as i32) {
            return true;
        }
        val = old as u32;
    }
}

/// `refcount_dec_and_mutex_lock()`: returns holding the mutex if able to
/// decrement the refcount to 0.
///
/// Similar to `atomic_dec_and_mutex_lock()`, it will WARN on underflow and fail
/// to decrement when saturated at `REFCOUNT_SATURATED`.
///
/// Provides release memory ordering, such that prior loads and stores are done
/// before, and provides a control dependency such that free() must come after.
/// See the comment on top of `include/linux/refcount.h`.
///
/// Returns true and holds the mutex if able to decrement the refcount to 0,
/// false otherwise.
///
/// # Safety
///
/// `r` points to a `refcount_t` on which the caller holds a reference, and
/// `lock` to an initialized mutex the caller may sleep on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn refcount_dec_and_mutex_lock(r: *mut refcount_t, lock: *mut mutex) -> bool {
    // SAFETY: (U4) the caller holds a reference on r, which is
    // refcount_dec_not_one()'s contract.
    if unsafe { refcount_dec_not_one(r) } {
        return false;
    }

    // SAFETY: (U1) mutex_lock(): lock is an initialized mutex, and the caller
    // may sleep.
    unsafe { c_refcount_mutex_lock(lock) };
    // SAFETY: (U1) refcount_dec_and_test(): the caller still holds its
    // reference on r.
    if !unsafe { c_refcount_dec_and_test(r) } {
        // SAFETY: (U1) mutex_unlock(): lock was locked above.
        unsafe { c_refcount_mutex_unlock(lock) };
        return false;
    }

    true
}

/// `refcount_dec_and_lock()`: returns holding the spinlock if able to
/// decrement the refcount to 0.
///
/// Similar to `atomic_dec_and_lock()`, it will WARN on underflow and fail to
/// decrement when saturated at `REFCOUNT_SATURATED`.
///
/// Provides release memory ordering, such that prior loads and stores are done
/// before, and provides a control dependency such that free() must come after.
/// See the comment on top of `include/linux/refcount.h`.
///
/// Returns true and holds the spinlock if able to decrement the refcount to 0,
/// false otherwise.
///
/// # Safety
///
/// `r` points to a `refcount_t` on which the caller holds a reference, and
/// `lock` to an initialized spinlock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn refcount_dec_and_lock(r: *mut refcount_t, lock: *mut spinlock_t) -> bool {
    // SAFETY: (U4) the caller holds a reference on r, which is
    // refcount_dec_not_one()'s contract.
    if unsafe { refcount_dec_not_one(r) } {
        return false;
    }

    // SAFETY: (U1) spin_lock(): lock is an initialized spinlock.
    unsafe { c_refcount_spin_lock(lock) };
    // SAFETY: (U1) refcount_dec_and_test(): the caller still holds its
    // reference on r.
    if !unsafe { c_refcount_dec_and_test(r) } {
        // SAFETY: (U1) spin_unlock(): lock was locked above.
        unsafe { c_refcount_spin_unlock(lock) };
        return false;
    }

    true
}

/// `refcount_dec_and_lock_irqsave()`: returns holding the spinlock with
/// disabled interrupts if able to decrement the refcount to 0.
///
/// Same as [`refcount_dec_and_lock()`] above except that the spinlock is
/// acquired with disabled interrupts.
///
/// Returns true and holds the spinlock if able to decrement the refcount to 0,
/// false otherwise.
///
/// # Safety
///
/// `r` points to a `refcount_t` on which the caller holds a reference, `lock`
/// to an initialized spinlock, and `flags` to the caller's saved IRQ flags.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn refcount_dec_and_lock_irqsave(
    r: *mut refcount_t,
    lock: *mut spinlock_t,
    flags: *mut c_ulong,
) -> bool {
    // SAFETY: (U4) the caller holds a reference on r, which is
    // refcount_dec_not_one()'s contract.
    if unsafe { refcount_dec_not_one(r) } {
        return false;
    }

    // SAFETY: (U1) spin_lock_irqsave(): lock is an initialized spinlock, and
    // flags points to the caller's unsigned long.
    unsafe { c_refcount_spin_lock_irqsave(lock, flags) };
    // SAFETY: (U1) refcount_dec_and_test(): the caller still holds its
    // reference on r.
    if !unsafe { c_refcount_dec_and_test(r) } {
        // SAFETY: (U1) spin_unlock_irqrestore(): lock was locked above, and
        // flags holds what spin_lock_irqsave() saved.
        unsafe { c_refcount_spin_unlock_irqrestore(lock, flags) };
        return false;
    }

    true
}
