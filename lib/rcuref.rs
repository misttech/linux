// SPDX-License-Identifier: GPL-2.0

//! rcuref - A scalable reference count implementation for RCU managed objects
//!
//! The Rust implementation of `lib/rcuref.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/rcuref.h` is the ABI contract, and its
//! static inlines stay in C. `lib/rcuref_ffi.c` exports these symbols with the
//! license of `lib/rcuref.c`, and wraps the C macros and inlines called from
//! here.
//!
//! rcuref is provided to replace open coded reference count implementations
//! based on `atomic_t`. It protects explicitely RCU managed objects which can
//! be visible even after the last reference has been dropped and the object
//! is heading towards destruction.
//!
//! A common usage pattern is:
//!
//! ```text
//! get()
//!        rcu_read_lock();
//!        p = get_ptr();
//!        if (p && !atomic_inc_not_zero(&p->refcnt))
//!                p = NULL;
//!        rcu_read_unlock();
//!        return p;
//!
//! put()
//!        if (!atomic_dec_return(&->refcnt)) {
//!                remove_ptr(p);
//!                kfree_rcu((p, rcu);
//!        }
//! ```
//!
//! `atomic_inc_not_zero()` is implemented with a `try_cmpxchg()` loop which has
//! O(N^2) behaviour under contention with N concurrent operations.
//!
//! rcuref uses `atomic_add_negative_relaxed()` for the fast path, which scales
//! better under contention.
//!
//! # Why not refcount?
//!
//! In principle it should be possible to make refcount use the rcuref
//! scheme, but the destruction race described below cannot be prevented
//! unless the protected object is RCU managed.
//!
//! # Theory of operation
//!
//! rcuref uses an unsigned integer reference counter. As long as the
//! counter value is greater than or equal to [`RCUREF_ONEREF`] and not larger
//! than [`RCUREF_MAXREF`] the reference is alive:
//!
//! ```text
//! ONEREF   MAXREF               SATURATED             RELEASED      DEAD    NOREF
//! 0        0x7FFFFFFF 0x8000000 0xA0000000 0xBFFFFFFF 0xC0000000 0xE0000000 0xFFFFFFFF
//! <---valid --------> <-------saturation zone-------> <-----dead zone----->
//! ```
//!
//! The `get()` and `put()` operations do unconditional increments and
//! decrements. The result is checked after the operation. This optimizes
//! for the fast path.
//!
//! If the reference count is saturated or dead, then the increments and
//! decrements are not harmful as the reference count still stays in the
//! respective zones and is always set back to STATURATED resp. DEAD. The
//! zones have room for 2^28 racing operations in each direction, which
//! makes it practically impossible to escape the zones.
//!
//! Once the last reference is dropped the reference count becomes
//! [`RCUREF_NOREF`] which forces `rcuref_put()` into the slowpath operation. The
//! slowpath then tries to set the reference count from `RCUREF_NOREF` to
//! [`RCUREF_DEAD`] via a `cmpxchg()`. This opens a small window where a
//! concurrent `rcuref_get()` can acquire the reference count and bring it
//! back to [`RCUREF_ONEREF`] or even drop the reference again and mark it DEAD.
//!
//! If the `cmpxchg()` succeeds then a concurrent `rcuref_get()` will result in
//! DEAD + 1, which is inside the dead zone. If that happens the reference
//! count is put back to DEAD.
//!
//! The actual race is possible due to the unconditional increment and
//! decrements in `rcuref_get()` and `rcuref_put()`:
//!
//! ```text
//!        T1                              T2
//!        get()                           put()
//!                                        if (atomic_add_negative(-1, &ref->refcnt))
//!                succeeds->                      atomic_cmpxchg(&ref->refcnt, NOREF, DEAD);
//!
//!        atomic_add_negative(1, &ref->refcnt);   <- Elevates refcount to DEAD + 1
//! ```
//!
//! As the result of T1's add is negative, the `get()` goes into the slow path
//! and observes refcnt being in the dead zone which makes the operation fail.
//!
//! Possible critical states:
//!
//! ```text
//!        Context Counter References      Operation
//!        T1      0       1               init()
//!        T2      1       2               get()
//!        T1      0       1               put()
//!        T2     -1       0               put() tries to mark dead
//!        T1      0       1               get()
//!        T2      0       1               put() mark dead fails
//!        T1     -1       0               put() tries to mark dead
//!        T1    DEAD      0               put() mark dead succeeds
//!        T2    DEAD+1    0               get() fails and puts it back to DEAD
//! ```
//!
//! Of course there are more complex scenarios, but the above illustrates
//! the working principle. The rest is left to the imagination of the
//! reader.
//!
//! # Deconstruction race
//!
//! The release operation must be protected by prohibiting a grace period in
//! order to prevent a possible use after free:
//!
//! ```text
//!        T1                              T2
//!        put()                           get()
//!        // ref->refcnt = ONEREF
//!        if (!atomic_add_negative(-1, &ref->refcnt))
//!                return false;                           <- Not taken
//!
//!        // ref->refcnt == NOREF
//!        --> preemption
//!                                        // Elevates ref->refcnt to ONEREF
//!                                        if (!atomic_add_negative(1, &ref->refcnt))
//!                                                return true;                    <- taken
//!
//!                                        if (put(&p->ref)) { <-- Succeeds
//!                                                remove_pointer(p);
//!                                                kfree_rcu(p, rcu);
//!                                        }
//!
//!                RCU grace period ends, object is freed
//!
//!        atomic_cmpxchg(&ref->refcnt, NOREF, DEAD);      <- UAF
//! ```
//!
//! This is prevented by disabling preemption around the `put()` operation as
//! that's in most kernel configurations cheaper than a `rcu_read_lock()` /
//! `rcu_read_unlock()` pair and in many cases even a NOOP. In any case it
//! prevents the grace period which keeps the object alive until all `put()`
//! operations complete.
//!
//! # Saturation protection
//!
//! The reference count has a saturation limit [`RCUREF_MAXREF`] (`INT_MAX`).
//! Once this is exceedded the reference count becomes stale by setting it
//! to [`RCUREF_SATURATED`], which will cause a memory leak, but it prevents
//! wrap arounds which obviously cause worse problems than a memory
//! leak. When saturation is reached a warning is emitted.
//!
//! # Race conditions
//!
//! All reference count increment/decrement operations are unconditional and
//! only verified after the fact. This optimizes for the good case and takes
//! the occasional race vs. a dead or already saturated refcount into
//! account. The saturation and dead zones are large enough to accomodate
//! for that.
//!
//! # Memory ordering
//!
//! Memory ordering rules are slightly relaxed wrt regular `atomic_t` functions
//! and provide only what is strictly required for refcounts.
//!
//! The increments are fully relaxed; these will not provide ordering. The
//! rationale is that whatever is used to obtain the object to increase the
//! reference count on will provide the ordering. For locked data
//! structures, its the lock acquire, for RCU/lockless data structures its
//! the dependent load.
//!
//! `rcuref_get()` provides a control dependency ordering future stores which
//! ensures that the object is not modified when acquiring a reference
//! fails.
//!
//! `rcuref_put()` provides release order, i.e. all prior loads and stores
//! will be issued before. It also provides a control dependency ordering
//! against the subsequent destruction of the object.
//!
//! If `rcuref_put()` successfully dropped the last reference and marked the
//! object DEAD it also provides acquire ordering.

use crate::refcount::{atomic_t, RefsAtomic};
use core::ffi::c_uint;

/// Mirror of `rcuref_t` (`include/linux/types.h`).
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct rcuref_t {
    refcnt: atomic_t,
}

kr::static_assert_layout!(rcuref_t, size = 4, align = 4, refcnt @ 0);

/// `RCUREF_ONEREF`: the counter value of one held reference.
pub const RCUREF_ONEREF: u32 = 0x00000000;
/// `RCUREF_MAXREF`: the last valid counter value.
pub const RCUREF_MAXREF: u32 = 0x7FFFFFFF;
/// `RCUREF_SATURATED`: the middle of the saturation zone.
pub const RCUREF_SATURATED: u32 = 0xA0000000;
/// `RCUREF_RELEASED`: the start of the dead zone.
pub const RCUREF_RELEASED: u32 = 0xC0000000;
/// `RCUREF_DEAD`: the middle of the dead zone.
pub const RCUREF_DEAD: u32 = 0xE0000000;
/// `RCUREF_NOREF`: the counter value after the last reference was dropped.
pub const RCUREF_NOREF: u32 = 0xFFFFFFFF;

extern "C" {
    // One wrapper per WARN_ONCE() site in lib/rcuref.c, so that each keeps its
    // own once flag.
    fn c_rcuref_warn_saturated();
    fn c_rcuref_warn_imbalanced_put();

    fn c_rcuref_acquire_after_ctrl_dep();
}

/// `rcuref_get_slowpath()`: slowpath of `rcuref_get()`.
///
/// Invoked when the reference count is outside of the valid zone.
///
/// Returns false if the reference count was already marked dead.
///
/// Returns true if the reference count is saturated, which prevents the
/// object from being deconstructed ever.
///
/// # Safety
///
/// `r` points to the `rcuref_t` that `rcuref_get()` in
/// `include/linux/rcuref.h` has just incremented.
#[no_mangle]
pub unsafe extern "C" fn rcuref_get_slowpath(r: *mut rcuref_t) -> bool {
    // SAFETY: (U3) rcuref_get() in include/linux/rcuref.h has just incremented
    // *r through the same pointer, so it points to a rcuref_t.
    let r = unsafe { &*r };

    get_slowpath(r.refcnt.counter(), || {
        // SAFETY: (U1) c_rcuref_warn_saturated() takes no arguments and only
        // WARN_ONCE()s.
        unsafe { c_rcuref_warn_saturated() }
    })
}

/// The body of [`rcuref_get_slowpath()`], generic over the atomic.
///
/// `warn_saturated` is the WARN_ONCE() on saturation.
pub(crate) fn get_slowpath(refs: &impl RefsAtomic, warn_saturated: impl FnOnce()) -> bool {
    let cnt = refs.read() as u32;

    // If the reference count was already marked dead, undo the
    // increment so it stays in the middle of the dead zone and return
    // fail.
    if cnt >= RCUREF_RELEASED {
        refs.set(RCUREF_DEAD as i32);
        return false;
    }

    // If it was saturated, warn and mark it so. In case the increment
    // was already on a saturated value restore the saturation
    // marker. This keeps it in the middle of the saturation zone and
    // prevents the reference count from overflowing. This leaks the
    // object memory, but prevents the obvious reference count overflow
    // damage.
    if cnt > RCUREF_MAXREF {
        warn_saturated();
        refs.set(RCUREF_SATURATED as i32);
    }
    true
}

/// `rcuref_put_slowpath()`: slowpath of `__rcuref_put()`.
///
/// `cnt` is the resulting value of the fastpath decrement.
///
/// Invoked when the reference count is outside of the valid zone.
///
/// Returns true if this was the last reference with no future references
/// possible. This signals the caller that it can safely schedule the
/// object, which is protected by the reference counter, for
/// deconstruction.
///
/// Returns false if there are still active references or the `put()` raced
/// with a concurrent `get()`/`put()` pair. Caller is not allowed to
/// deconstruct the protected object.
///
/// # Safety
///
/// `r` points to the `rcuref_t` that `__rcuref_put()` in
/// `include/linux/rcuref.h` has just decremented to `cnt`, in a context that
/// prohibits a grace period, so the object is alive for this call.
#[no_mangle]
pub unsafe extern "C" fn rcuref_put_slowpath(r: *mut rcuref_t, cnt: c_uint) -> bool {
    // SAFETY: (U5) __rcuref_put() in include/linux/rcuref.h has just
    // decremented *r through the same pointer, and holds off the grace period
    // that would free the object, so it points to a live rcuref_t.
    let r = unsafe { &*r };

    put_slowpath(
        r.refcnt.counter(),
        cnt,
        || {
            // SAFETY: (U1) c_rcuref_acquire_after_ctrl_dep() takes no arguments
            // and only issues smp_acquire__after_ctrl_dep().
            unsafe { c_rcuref_acquire_after_ctrl_dep() }
        },
        || {
            // SAFETY: (U1) c_rcuref_warn_imbalanced_put() takes no arguments
            // and only WARN_ONCE()s.
            unsafe { c_rcuref_warn_imbalanced_put() }
        },
    )
}

/// The body of [`rcuref_put_slowpath()`], generic over the atomic.
///
/// `acquire_after_ctrl_dep` is `smp_acquire__after_ctrl_dep()`, and
/// `warn_imbalanced` the WARN_ONCE() on an imbalanced put.
pub(crate) fn put_slowpath(
    refs: &impl RefsAtomic,
    cnt: u32,
    acquire_after_ctrl_dep: impl FnOnce(),
    warn_imbalanced: impl FnOnce(),
) -> bool {
    // Did this drop the last reference?
    if cnt == RCUREF_NOREF {
        // Carefully try to set the reference count to RCUREF_DEAD.
        //
        // This can fail if a concurrent get() operation has
        // elevated it again or the corresponding put() even marked
        // it dead already. Both are valid situations and do not
        // require a retry. If this fails the caller is not
        // allowed to deconstruct the object.
        let mut old = cnt as i32;
        if !refs.try_cmpxchg_release(&mut old, RCUREF_DEAD as i32) {
            return false;
        }

        // The caller can safely schedule the object for
        // deconstruction. Provide acquire ordering.
        acquire_after_ctrl_dep();
        return true;
    }

    // If the reference count was already in the dead zone, then this
    // put() operation is imbalanced. Warn, put the reference count back to
    // DEAD and tell the caller to not deconstruct the object.
    if cnt >= RCUREF_RELEASED {
        warn_imbalanced();
        refs.set(RCUREF_DEAD as i32);
        return false;
    }

    // This is a put() operation on a saturated refcount. Restore the
    // mean saturation value and tell the caller to not deconstruct the
    // object.
    if cnt > RCUREF_MAXREF {
        refs.set(RCUREF_SATURATED as i32);
    }
    false
}
