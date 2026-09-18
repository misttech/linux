// SPDX-License-Identifier: GPL-2.0

//! Locked reference counts, replacing `lib/lockref.c` in place.
//!
//! The C header remains the ABI contract. The spinlock is opaque; Kbuild
//! measures its enclosing layout for each configuration before compiling this
//! mirror. C callers may hold references that Rust cannot see.

use core::cell::UnsafeCell;
use core::ffi::{c_int, c_void};
use core::ptr;

use crate::port_layout as layout;

/// `struct lockref` from `include/linux/lockref.h`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct lockref {
    // A zero-sized marker gives this mirror the C compiler's alignment.
    _align: [layout::LockrefAlign; 0],
    lock: kr::OpaqueBytes<{ layout::PORT_LOCKREF_COUNT }>,
    count: UnsafeCell<c_int>,
    _tail: kr::OpaqueBytes<{ layout::PORT_LOCKREF_SIZE - layout::PORT_LOCKREF_COUNT - 4 }>,
}

kr::static_assert_layout!(
    lockref,
    size = layout::PORT_LOCKREF_SIZE,
    align = layout::PORT_LOCKREF_ALIGN,
    _align @ 0,
    lock @ layout::PORT_LOCKREF_LOCK,
    count @ layout::PORT_LOCKREF_COUNT,
    _tail @ (layout::PORT_LOCKREF_COUNT + 4),
);
kr::static_assert!(layout::PORT_LOCKREF_FAST == cfg!(PORT_LOCKREF_FAST) as usize);
kr::static_assert!(!cfg!(PORT_LOCKREF_FAST) || layout::PORT_LOCKREF_LOCK_COUNT == 0);
kr::static_assert!(!cfg!(PORT_LOCKREF_FAST) || layout::PORT_LOCKREF_SIZE == 8);
kr::static_assert!(!cfg!(PORT_LOCKREF_FAST) || layout::PORT_LOCKREF_ALIGN == 8);

unsafe extern "C" {
    fn c_lockref_spin_lock(lockref: *mut c_void);
    fn c_lockref_spin_unlock(lockref: *mut c_void);
    fn c_lockref_assert_spin_locked(lockref: *mut c_void);

    #[cfg(PORT_LOCKREF_FAST)]
    fn c_lockref_read_lock_count(lockref: *mut c_void) -> u64;
    #[cfg(PORT_LOCKREF_FAST)]
    fn c_lockref_arch_unlocked(snapshot: *const c_void) -> bool;
    #[cfg(PORT_LOCKREF_FAST)]
    fn c_lockref_try_cmpxchg(lockref: *mut c_void, old: *mut u64, new: u64) -> bool;
}

/// Read the C count under the associated spinlock.
fn read_count(value: *mut lockref) -> c_int {
    // SAFETY: (U3) the caller holds lockref.lock, or is the C caller of
    // lockref_mark_dead() which holds that lock. The C field is live.
    unsafe { ptr::read(ptr::addr_of!((*value).count).cast::<c_int>()) }
}

/// Write the C count under the associated spinlock.
fn write_count(value: *mut lockref, count: c_int) {
    // SAFETY: (U3) as read_count(): the lock is held and the C field is live.
    unsafe { ptr::write(ptr::addr_of_mut!((*value).count).cast::<c_int>(), count) }
}

#[cfg(PORT_LOCKREF_FAST)]
#[derive(Clone, Copy)]
enum FastResult {
    Updated(c_int),
    Rejected,
    Contended,
}

/// Read `old.count` from a snapshot of the eight-byte C union.
#[cfg(PORT_LOCKREF_FAST)]
fn snapshot_count(word: u64) -> c_int {
    let bytes = word.to_ne_bytes();
    // SAFETY: (U2) the generated C offset and the size/align assertions
    // locate count inside the eight-byte lockref snapshot.
    unsafe {
        ptr::read_unaligned(
            bytes
                .as_ptr()
                .add(layout::PORT_LOCKREF_COUNT)
                .cast::<c_int>(),
        )
    }
}

/// Set `new.count` in a local snapshot without touching the live C object.
#[cfg(PORT_LOCKREF_FAST)]
fn snapshot_with_count(word: u64, count: c_int) -> u64 {
    let mut bytes = word.to_ne_bytes();
    // SAFETY: (U2) as snapshot_count(): count occupies these four bytes in
    // the C union. This writes only the local snapshot.
    unsafe {
        ptr::write_unaligned(
            bytes
                .as_mut_ptr()
                .add(layout::PORT_LOCKREF_COUNT)
                .cast::<c_int>(),
            count,
        )
    };
    u64::from_ne_bytes(bytes)
}

/// The C `CMPXCHG_LOOP`: 100 attempts while the snapshot lock is unlocked.
#[cfg(PORT_LOCKREF_FAST)]
fn fast_update(value: *mut lockref, delta: c_int, allowed: impl Fn(c_int) -> bool) -> FastResult {
    // SAFETY: (U1) READ_ONCE on the live C lockref union.
    // As in the C macro, a failed cmpxchg reloads `old` for the next try.
    let mut old = unsafe { c_lockref_read_lock_count(value.cast()) };
    let mut retry = 100;
    loop {
        // SAFETY: (U1) the C wrapper reads the raw lock from this local
        // snapshot, using the architecture's unlocked test.
        if !unsafe { c_lockref_arch_unlocked((&old as *const u64).cast::<c_void>()) } {
            break;
        }
        let count = snapshot_count(old);
        if !allowed(count) {
            return FastResult::Rejected;
        }
        let next = count.wrapping_add(delta);
        let new = snapshot_with_count(old, next);
        // SAFETY: (U1) try_cmpxchg64_relaxed updates old on failure, just as
        // the C macro does. The C contract provides a live lockref pointer.
        if unsafe { c_lockref_try_cmpxchg(value.cast(), &mut old, new) } {
            return FastResult::Updated(next);
        }
        retry -= 1;
        if retry == 0 {
            break;
        }
    }
    FastResult::Contended
}

/// `CMPXCHG_LOOP` is empty when the C configuration disables it.
#[cfg(not(PORT_LOCKREF_FAST))]
#[allow(dead_code)]
#[derive(Clone, Copy)]
enum FastResult {
    Updated(c_int),
    Rejected,
    Contended,
}

#[cfg(not(PORT_LOCKREF_FAST))]
fn fast_update(
    _value: *mut lockref,
    _delta: c_int,
    _allowed: impl Fn(c_int) -> bool,
) -> FastResult {
    FastResult::Contended
}

/// Increments the count unconditionally. The caller already holds a reference.
///
/// # Safety
///
/// `value` is a live, initialized C `struct lockref`. The caller holds a
/// reference, so the count cannot become zero during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lockref_get(value: *mut lockref) {
    if let FastResult::Updated(_) = fast_update(value, 1, |_| true) {
        return;
    }
    // SAFETY: (U1) spin_lock() serializes the C lockref count fallback.
    unsafe { c_lockref_spin_lock(value.cast()) };
    write_count(value, read_count(value).wrapping_add(1));
    // SAFETY: (U1) releases the lock taken above.
    unsafe { c_lockref_spin_unlock(value.cast()) };
}

/// Increments unless the count is zero or dead.
///
/// # Safety
///
/// `value` is a live, initialized C `struct lockref`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lockref_get_not_zero(value: *mut lockref) -> bool {
    match fast_update(value, 1, |count| count > 0) {
        FastResult::Updated(_) => return true,
        FastResult::Rejected => return false,
        FastResult::Contended => {}
    }
    // SAFETY: (U1) spin_lock() serializes the C lockref count fallback.
    unsafe { c_lockref_spin_lock(value.cast()) };
    let mut retval = false;
    let count = read_count(value);
    if count > 0 {
        write_count(value, count.wrapping_add(1));
        retval = true;
    }
    // SAFETY: (U1) releases the lock taken above.
    unsafe { c_lockref_spin_unlock(value.cast()) };
    retval
}

/// Decrements if the fast path can do so, returning the new count or -1.
///
/// # Safety
///
/// `value` is a live, initialized C `struct lockref`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lockref_put_return(value: *mut lockref) -> c_int {
    if let FastResult::Updated(count) = fast_update(value, -1, |count| count > 0) {
        return count;
    }
    -1
}

/// Decrements unless count is at most one; otherwise returns with lock held.
///
/// # Safety
///
/// `value` is a live, initialized C `struct lockref`. When this returns false,
/// the caller owns `value.lock` and must release it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lockref_put_or_lock(value: *mut lockref) -> bool {
    if let FastResult::Updated(_) = fast_update(value, -1, |count| count > 1) {
        return true;
    }
    // SAFETY: (U1) spin_lock() begins the C lockref count fallback.
    unsafe { c_lockref_spin_lock(value.cast()) };
    let count = read_count(value);
    if count <= 1 {
        return false;
    }
    write_count(value, count.wrapping_sub(1));
    // SAFETY: (U1) releases the lock taken above on the true path.
    unsafe { c_lockref_spin_unlock(value.cast()) };
    true
}

/// Marks a lockref dead. The C caller must hold its spinlock.
///
/// # Safety
///
/// `value` is a live, initialized C `struct lockref`, and its lock is held.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lockref_mark_dead(value: *mut lockref) {
    // SAFETY: (U1) the C contract requires the caller to hold value.lock.
    unsafe { c_lockref_assert_spin_locked(value.cast()) };
    write_count(value, layout::PORT_LOCKREF_DEAD_VAL);
}

/// Increments unless the count is dead (negative).
///
/// # Safety
///
/// `value` is a live, initialized C `struct lockref`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lockref_get_not_dead(value: *mut lockref) -> bool {
    match fast_update(value, 1, |count| count >= 0) {
        FastResult::Updated(_) => return true,
        FastResult::Rejected => return false,
        FastResult::Contended => {}
    }
    // SAFETY: (U1) spin_lock() serializes the C lockref count fallback.
    unsafe { c_lockref_spin_lock(value.cast()) };
    let mut retval = false;
    let count = read_count(value);
    if count >= 0 {
        write_count(value, count.wrapping_add(1));
        retval = true;
    }
    // SAFETY: (U1) releases the lock taken above.
    unsafe { c_lockref_spin_unlock(value.cast()) };
    retval
}
