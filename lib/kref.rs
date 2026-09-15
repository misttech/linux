// SPDX-License-Identifier: GPL-2.0-only

//! Generic reference counted objects.
//!
//! The typed Rust API of `include/linux/kref.h`. That header is header-only:
//! its static inlines stay in C for C callers, and this file gives Rust the
//! same operations on the same embedded `struct kref`.
//!
//! A [`KRef<T>`] is one counted reference to a `T` that embeds a [`Kref`]. C
//! may hold references that Rust cannot see, so a `KRef` never reasons about
//! uniqueness and never hands out `&mut T`. It is not an `Arc`: the count is
//! the one embedded in `T`, and [`KrefCounted::release()`] decides what
//! happens to the memory.

use crate::refcount::{
    mutex, refcount_dec_and_lock, refcount_dec_and_mutex_lock, refcount_t, spinlock_t,
};
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::ptr::{self, NonNull};

/// Mirror of `struct kref`.
#[repr(C)]
pub struct Kref {
    refcount: refcount_t,
}

kr::static_assert_layout!(Kref, size = 4, align = 4, refcount @ 0);

impl Kref {
    /// `KREF_INIT(1)`, the value `kref_init()` sets: a new object holds one
    /// reference.
    pub const fn new() -> Self {
        Self {
            refcount: refcount_t::new(1),
        }
    }

    /// `kref_read()`.
    #[inline]
    pub fn read(&self) -> u32 {
        self.refcount.read()
    }

    /// The embedded `refcount_t`, which C reaches as `kref->refcount`.
    ///
    /// # Safety
    ///
    /// Changes made through the returned `refcount_t` keep the count equal to
    /// the number of references to the object. Setting or decrementing it
    /// without giving up references would let a [`KRef`] outlive the object.
    #[inline]
    pub unsafe fn refcount(&self) -> &refcount_t {
        &self.refcount
    }
}

impl Default for Kref {
    fn default() -> Self {
        Self::new()
    }
}

extern "C" {
    fn c_kref_mutex_unlock(lock: *mut mutex);
    fn c_kref_spin_unlock(lock: *mut spinlock_t);
}

/// An object that embeds a [`Kref`] counting the references to it.
///
/// # Safety
///
/// - [`kref()`](Self::kref) always returns the same [`Kref`], embedded in the
///   object, and that count covers every reference to the object, from Rust
///   and from C.
/// - [`release()`](Self::release) is the object's release function: it runs
///   once, when the count reaches zero.
pub unsafe trait KrefCounted {
    /// The embedded `struct kref`.
    fn kref(&self) -> &Kref;

    /// Releases the object after its last reference is dropped, like the
    /// `release` function passed to `kref_put()`.
    ///
    /// # Safety
    ///
    /// The object's count has reached zero, and nothing uses the object after
    /// this call.
    unsafe fn release(this: NonNull<Self>);
}

/// A C `struct mutex` that stays valid for `'a`.
#[derive(Clone, Copy)]
pub struct MutexRef<'a> {
    ptr: NonNull<mutex>,
    _lock: PhantomData<&'a mutex>,
}

impl MutexRef<'_> {
    /// Wraps a C mutex.
    ///
    /// # Safety
    ///
    /// `ptr` points to an initialized `struct mutex` that stays valid for the
    /// returned lifetime.
    pub unsafe fn from_raw(ptr: NonNull<mutex>) -> Self {
        Self {
            ptr,
            _lock: PhantomData,
        }
    }
}

// SAFETY: (U6) a struct mutex is locked and unlocked from any task, and the
// reference only names it.
unsafe impl Send for MutexRef<'_> {}

// SAFETY: (U6) sharing the reference only lets tasks lock the mutex, which
// serializes them.
unsafe impl Sync for MutexRef<'_> {}

/// A C `spinlock_t` that stays valid for `'a`.
#[derive(Clone, Copy)]
pub struct SpinLockRef<'a> {
    ptr: NonNull<spinlock_t>,
    _lock: PhantomData<&'a spinlock_t>,
}

impl SpinLockRef<'_> {
    /// Wraps a C spinlock.
    ///
    /// # Safety
    ///
    /// `ptr` points to an initialized `spinlock_t` that stays valid for the
    /// returned lifetime.
    pub unsafe fn from_raw(ptr: NonNull<spinlock_t>) -> Self {
        Self {
            ptr,
            _lock: PhantomData,
        }
    }
}

// SAFETY: (U6) a spinlock_t is locked and unlocked from any CPU, and the
// reference only names it.
unsafe impl Send for SpinLockRef<'_> {}

// SAFETY: (U6) sharing the reference only lets CPUs lock the spinlock, which
// serializes them.
unsafe impl Sync for SpinLockRef<'_> {}

/// One counted reference to a `T`.
///
/// Cloning takes another reference (`kref_get()`), and dropping puts it
/// (`kref_put()`), calling [`KrefCounted::release()`] when it was the last.
pub struct KRef<T: KrefCounted> {
    ptr: NonNull<T>,
    _ref: PhantomData<T>,
}

// SAFETY: (U6) the count is atomic, and C takes and puts references from any
// CPU, so a reference may move to another thread when T may be shared with
// one.
unsafe impl<T: KrefCounted + Send + Sync> Send for KRef<T> {}

// SAFETY: (U6) sharing a KRef<T> shares &T and lets other threads take
// references, which are atomic increments, so it needs T: Send + Sync.
unsafe impl<T: KrefCounted + Send + Sync> Sync for KRef<T> {}

impl<T: KrefCounted> KRef<T> {
    /// `kref_get()`: takes a new reference on `obj`.
    ///
    /// A count of zero saturates and warns, as `refcount_inc()` does.
    #[inline]
    pub fn get(obj: &T) -> Self {
        obj.kref().refcount.inc();

        Self {
            ptr: NonNull::from(obj),
            _ref: PhantomData,
        }
    }

    /// `kref_get_unless_zero()`: takes a new reference on `obj` unless its
    /// count is zero.
    ///
    /// For objects that can be looked up from a lookup structure, and which
    /// are removed from that lookup structure when released: `None` means the
    /// lookup found an object that is being released.
    #[inline]
    pub fn get_unless_zero(obj: &T) -> Option<Self> {
        if !obj.kref().refcount.inc_not_zero() {
            return None;
        }

        Some(Self {
            ptr: NonNull::from(obj),
            _ref: PhantomData,
        })
    }

    /// Takes over a counted reference, for instance one that C hands to Rust.
    ///
    /// # Safety
    ///
    /// `ptr` points to a live `T`, and the caller gives up one counted
    /// reference to it.
    #[inline]
    pub unsafe fn from_raw(ptr: NonNull<T>) -> Self {
        Self {
            ptr,
            _ref: PhantomData,
        }
    }

    /// Gives up the reference without putting it, for instance to hand it to
    /// C, which then owns it.
    #[inline]
    pub fn into_raw(this: Self) -> NonNull<T> {
        ManuallyDrop::new(this).ptr
    }

    /// `kref_read()` of the referenced object.
    #[inline]
    pub fn read(this: &Self) -> u32 {
        this.kref().read()
    }

    /// `kref_put_mutex()`: puts the reference and, if it was the last, removes
    /// the object with `unlink` under `lock` before releasing it.
    ///
    /// In C, `release` runs with the mutex held and unlocks it. Here `unlink`
    /// runs with the mutex held, the mutex is unlocked, and then the object is
    /// released. A lookup that takes references with
    /// [`get_unless_zero()`](Self::get_unless_zero) under the same mutex
    /// therefore never finds a released object.
    ///
    /// Returns true if this call released the object.
    pub fn put_mutex(this: Self, lock: MutexRef<'_>, unlink: impl FnOnce(&T)) -> bool {
        let obj_ptr = Self::into_raw(this);
        // SAFETY: (U4) `this` held a reference, so the object is live until its
        // count reaches zero and it is released below.
        let obj = unsafe { obj_ptr.as_ref() };
        let refcount = ptr::from_ref(&obj.kref().refcount).cast_mut();

        // SAFETY: (U4) the reference given up above is the one this drops, and
        // lock points to a live mutex, as MutexRef guarantees.
        if !unsafe { refcount_dec_and_mutex_lock(refcount, lock.ptr.as_ptr()) } {
            return false;
        }

        unlink(obj);
        // SAFETY: (U1) mutex_unlock(): refcount_dec_and_mutex_lock() returned
        // true, holding lock.
        unsafe { c_kref_mutex_unlock(lock.ptr.as_ptr()) };
        // SAFETY: (U4) the count reached zero, and obj is not used again.
        unsafe { T::release(obj_ptr) };
        true
    }

    /// `kref_put_lock()`: puts the reference and, if it was the last, removes
    /// the object with `unlink` under `lock` before releasing it.
    ///
    /// In C, `release` runs with the spinlock held and unlocks it. Here
    /// `unlink` runs with the spinlock held, the spinlock is unlocked, and then
    /// the object is released. A lookup that takes references with
    /// [`get_unless_zero()`](Self::get_unless_zero) under the same spinlock
    /// therefore never finds a released object.
    ///
    /// Returns true if this call released the object.
    pub fn put_lock(this: Self, lock: SpinLockRef<'_>, unlink: impl FnOnce(&T)) -> bool {
        let obj_ptr = Self::into_raw(this);
        // SAFETY: (U4) `this` held a reference, so the object is live until its
        // count reaches zero and it is released below.
        let obj = unsafe { obj_ptr.as_ref() };
        let refcount = ptr::from_ref(&obj.kref().refcount).cast_mut();

        // SAFETY: (U4) the reference given up above is the one this drops, and
        // lock points to a live spinlock, as SpinLockRef guarantees.
        if !unsafe { refcount_dec_and_lock(refcount, lock.ptr.as_ptr()) } {
            return false;
        }

        unlink(obj);
        // SAFETY: (U1) spin_unlock(): refcount_dec_and_lock() returned true,
        // holding lock.
        unsafe { c_kref_spin_unlock(lock.ptr.as_ptr()) };
        // SAFETY: (U4) the count reached zero, and obj is not used again.
        unsafe { T::release(obj_ptr) };
        true
    }
}

impl<T: KrefCounted> Clone for KRef<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self::get(self)
    }
}

impl<T: KrefCounted> Deref for KRef<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: (U4) self holds a reference, so the object is live for as
        // long as self is borrowed.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: KrefCounted> Drop for KRef<T> {
    #[inline]
    fn drop(&mut self) {
        if self.kref().refcount.dec_and_test() {
            // SAFETY: (U4) that was the last reference, and self is not used
            // after it is dropped.
            unsafe { T::release(self.ptr) };
        }
    }
}
