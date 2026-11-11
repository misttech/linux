// SPDX-License-Identifier: GPL-2.0-only

//! `RcuPtr<P>` and `RcuReadPtr<T>`: a typed Rust API over the RCU pointer primitives of
//! `include/linux/rcupdate.h`, for a pointer that C declares `__rcu`.
//!
//! RCU lets readers follow a pointer with no lock while a writer replaces it, and frees
//! the old object only after every reader that could have seen it has left its read-side
//! critical section (a grace period). The C rules are conventions: `rcu_dereference()`
//! only between `rcu_read_lock()` and `rcu_read_unlock()`, `rcu_assign_pointer()` only
//! after the object is initialized, and a free only after a grace period. Nothing checks
//! them but lockdep, at run time and only in a debug kernel. This API encodes them in
//! types:
//!
//! - [`RcuGuard`] is the read-side critical section. It is created by [`rcu_read_lock()`],
//!   ends when it is dropped, and cannot leave its thread.
//! - [`RcuPtr::read()`] takes a guard, and gives a reference that lives as long as the
//!   guard: the borrow checker stops it from outliving the critical section, which
//!   `rcu_dereference()` cannot.
//! - [`RcuPtr::replace()`] publishes a new owner and returns the old one as a
//!   [`Retired`]. Dropping a `Retired` queues its owner to be dropped after a grace
//!   period, with `call_rcu()`: it never waits, never sleeps, and cannot free what a
//!   reader still holds, on any RCU configuration. For that the object embeds a
//!   `struct rcu_head` ([`HasRcuHead`]), as C objects that `kfree_rcu()` do. Getting
//!   the owner back, [`Retired::synchronize()`], waits for the grace period and is
//!   `unsafe`: it must not be called in a critical section, where on some
//!   configurations `synchronize_rcu()` returns at once.
//!
//! The memory is the C's: both types are `#[repr(transparent)]` over the pointer that
//! `struct foo { struct bar __rcu *ptr; }` has, and C keeps using `rcu_dereference()` and
//! `rcu_assign_pointer()` on it. [`RcuReadPtr`] is the read-only view of a word that C
//! owns and updates, the common case for a leaf; [`RcuPtr`] is the word of which Rust is
//! the updater and owns the object. The port never hands out `&mut`, and reasons about no
//! uniqueness: C may hold a reference that Rust cannot see.
//!
//! Ordering. `rcu_assign_pointer()` is `smp_store_release()`, and the Rust store is a
//! `Release` too. `rcu_dereference()` is a `READ_ONCE()` that C orders by the address
//! dependency of what is loaded through it, which Rust cannot express: the port loads
//! with `Acquire`. That is stronger everywhere and free on x86, but it is a load-acquire
//! (`ldar`) where arm64 C has a plain load (`ldr`), so **the dependency ordering is
//! lost, and the cost is a real one on arm64**. See `units/rcu_ptr.md`.
//!
//! `rcu_read_lock()`, `rcu_read_unlock()`, `synchronize_rcu()` and `call_rcu()` are C's,
//! reached through [`RcuDomain`], a sealed trait, so that a model of the grace period can
//! stand in for them and the port's own protocol runs under loom.

use core::marker::PhantomData;
use core::mem::{self, ManuallyDrop};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::kref::{KRef, KrefCounted};

unsafe extern "C" {
    /// `rcu_read_lock()`, a static inline.
    fn c_rcu_read_lock();
    /// `rcu_read_unlock()`, a static inline.
    fn c_rcu_read_unlock();
    /// `synchronize_rcu()`: waits for every reader that started before it.
    fn synchronize_rcu();
    /// `call_rcu()`: calls `func(head)` after a grace period.
    fn call_rcu(head: *mut RcuHead, func: unsafe extern "C" fn(*mut RcuHead));
}

/// `struct rcu_head`, which is `struct callback_head`: what `call_rcu()` queues an
/// object by. C owns its two pointers, so they are opaque here.
#[repr(C)]
pub struct RcuHead {
    next: kr::Opaque<*mut RcuHead>,
    func: kr::Opaque<Option<unsafe extern "C" fn(*mut RcuHead)>>,
}

// BTF of x86_64 kernels: `callback_head` is two pointers. 32-bit values are from DWARF of
// clang --target=i386 for the same struct (include/linux/types.h). Neither depends on the
// configuration.
#[cfg(target_pointer_width = "64")]
kr::static_assert_layout!(RcuHead, size = 16, align = 8, next @ 0, func @ 8);
#[cfg(target_pointer_width = "32")]
kr::static_assert_layout!(RcuHead, size = 8, align = 4, next @ 0, func @ 4);

// SAFETY: (U6) only C touches the two pointers, and only `call_rcu()` and the grace period
// do, one queueing at a time: an object's head is used by nothing else (HasRcuHead).
unsafe impl Sync for RcuHead {}

// SAFETY: (U6) as above; the head is data of the object, which moves between threads as its
// owner does.
unsafe impl Send for RcuHead {}

impl RcuHead {
    /// A head that is not queued. `call_rcu()` initializes it.
    pub const fn new() -> Self {
        Self {
            next: kr::Opaque::uninit(),
            func: kr::Opaque::uninit(),
        }
    }
}

impl Default for RcuHead {
    fn default() -> Self {
        Self::new()
    }
}

/// An object that embeds a [`RcuHead`] for `call_rcu()` to queue it by.
///
/// # Safety
///
/// `OFFSET` is the offset of an `RcuHead` field of `Self`, found with
/// `core::mem::offset_of!`, and nothing else uses that head. The
/// [`impl_has_rcu_head!`] macro writes this impl and checks the field's type.
pub unsafe trait HasRcuHead {
    /// The offset of the embedded `RcuHead`.
    const OFFSET: usize;
}

/// Implement [`HasRcuHead`] for a field of a type, which must not be generic.
///
/// `impl_has_rcu_head!(Config, rcu)`.
#[macro_export]
macro_rules! impl_has_rcu_head {
    ($ty:ty, $field:ident) => {
        // SAFETY: (U2) the offset is that of the field, and the field is checked to be an
        // RcuHead below.
        unsafe impl $crate::rcu_ptr::HasRcuHead for $ty {
            const OFFSET: usize = ::core::mem::offset_of!($ty, $field);
        }
        const _: fn(&$ty) -> &$crate::rcu_ptr::RcuHead = |x| &x.$field;
    };
}

pub(crate) mod sealed {
    /// What only the kernel's RCU and the models of this crate implement.
    pub trait Sealed {}
}

/// What a grace period is made of, in the calls that only its implementations make.
///
/// The kernel's is [`Kernel`]. Every method is an associated function, because RCU is
/// global state. The trait is sealed and its methods are `unsafe`: a safe call of
/// `read_unlock()` under a live [`RcuGuard`] would end its critical section, and a call
/// of `synchronize()` in one may return at once.
pub trait RcuDomain: sealed::Sealed {
    /// Enters a read-side critical section: `rcu_read_lock()`.
    ///
    /// # Safety
    ///
    /// The caller pairs it with [`read_unlock()`](Self::read_unlock) on the same thread.
    unsafe fn read_lock();

    /// Leaves it: `rcu_read_unlock()`.
    ///
    /// # Safety
    ///
    /// The caller entered a critical section with `read_lock()` on this thread, and
    /// leaves it once.
    unsafe fn read_unlock();

    /// Returns after every read-side critical section that was in progress at the call has
    /// ended: `synchronize_rcu()`. May sleep.
    ///
    /// # Safety
    ///
    /// The caller is not in a read-side critical section, and may sleep.
    unsafe fn synchronize();

    /// Calls `func(head)` after a grace period: `call_rcu()`.
    ///
    /// # Safety
    ///
    /// `head` is the head of an object that `func` frees or drops, and no other use of
    /// the head is queued. `func` is sound to call with it after a grace period, in the
    /// context that `call_rcu()` runs callbacks in.
    unsafe fn call_rcu(head: *mut RcuHead, func: unsafe extern "C" fn(*mut RcuHead));
}

/// The kernel's RCU.
pub struct Kernel;

impl sealed::Sealed for Kernel {}

impl RcuDomain for Kernel {
    unsafe fn read_lock() {
        // SAFETY: (U1) rcu_read_lock(): takes nothing, and the caller pairs it.
        unsafe { c_rcu_read_lock() }
    }

    unsafe fn read_unlock() {
        // SAFETY: (U1) rcu_read_unlock(): the caller entered with read_lock().
        unsafe { c_rcu_read_unlock() }
    }

    unsafe fn synchronize() {
        // SAFETY: (U1) synchronize_rcu(): takes nothing; the caller may sleep and holds no
        // critical section.
        unsafe { synchronize_rcu() }
    }

    unsafe fn call_rcu(head: *mut RcuHead, func: unsafe extern "C" fn(*mut RcuHead)) {
        // SAFETY: (U1) call_rcu(): the caller's contract for head and func.
        unsafe { call_rcu(head, func) }
    }
}

/// An RCU read-side critical section: from [`rcu_read_lock()`] until the guard is
/// dropped. It is the proof that [`RcuPtr::read()`] asks for.
///
/// It is neither `Send` nor `Sync`: `rcu_read_unlock()` runs where `rcu_read_lock()` did,
/// and a critical section on one CPU says nothing about another. Critical sections nest.
/// Code inside one must not sleep, which the type does not check.
#[must_use = "the read-side critical section ends when the guard is dropped"]
pub struct RcuGuard<D: RcuDomain = Kernel> {
    _not_send: PhantomData<*mut D>,
}

impl<D: RcuDomain> RcuGuard<D> {
    /// Enters a read-side critical section.
    pub fn enter() -> Self {
        // SAFETY: (U1) the guard is not Send, so its Drop runs on this thread, and
        // read_unlock() pairs this call.
        unsafe { D::read_lock() };
        Self {
            _not_send: PhantomData,
        }
    }
}

impl<D: RcuDomain> Drop for RcuGuard<D> {
    fn drop(&mut self) {
        // SAFETY: (U1) enter() called read_lock() on this thread, and this is the one drop.
        unsafe { D::read_unlock() };
    }
}

/// `rcu_read_lock()`: enters a read-side critical section of the kernel's RCU.
pub fn rcu_read_lock() -> RcuGuard {
    RcuGuard::enter()
}

/// An owning pointer to the object of an [`RcuPtr`].
///
/// `Send`, because the owner is dropped by `call_rcu()`'s callback, on another CPU.
///
/// # Safety
///
/// [`into_raw()`](Self::into_raw) gives up one owner of the object, which stays valid
/// until [`from_raw()`](Self::from_raw) takes that owner back, and dropping the owner
/// after a grace period frees no memory that a reader can still reach: the object is not
/// touched by anything else once its last owner is dropped.
pub unsafe trait RcuOwner: Sized + Send {
    /// The object the pointer owns, with the `rcu_head` that queues its release.
    type Target: HasRcuHead;

    /// Gives up ownership of the object.
    fn into_raw(self) -> NonNull<Self::Target>;

    /// Takes back an object that [`into_raw()`](Self::into_raw) gave up.
    ///
    /// # Safety
    ///
    /// `ptr` comes from `into_raw()`, and is taken back only once.
    unsafe fn from_raw(ptr: NonNull<Self::Target>) -> Self;
}

// SAFETY: (U4) a KRef owns one reference, which into_raw() hands to the RcuPtr and
// from_raw() takes back; the object stays live while the reference is held, and its
// release runs when the last reference is dropped.
unsafe impl<T: KrefCounted + HasRcuHead + Send + Sync> RcuOwner for KRef<T> {
    type Target = T;

    fn into_raw(self) -> NonNull<T> {
        KRef::into_raw(self)
    }

    unsafe fn from_raw(ptr: NonNull<T>) -> Self {
        // SAFETY: (U4) the caller's contract is the one of KRef::from_raw().
        unsafe { KRef::from_raw(ptr) }
    }
}

/// The atomic word a pointer is kept in: the seam that lets a loom model run the protocol
/// on loom's atomics. It only forwards, so the orderings are chosen by the port's own
/// functions below, and the model tests those.
pub(crate) trait Atomic<T> {
    fn load(&self, order: Ordering) -> *mut T;
    fn swap(&self, new: *mut T, order: Ordering) -> *mut T;
}

impl<T> Atomic<T> for AtomicPtr<T> {
    fn load(&self, order: Ordering) -> *mut T {
        AtomicPtr::load(self, order)
    }

    fn swap(&self, new: *mut T, order: Ordering) -> *mut T {
        AtomicPtr::swap(self, new, order)
    }
}

/// An owner that has been unpublished, and that readers may still hold a reference to.
///
/// Dropping it queues the owner to be dropped after a grace period, with `call_rcu()`:
/// no wait, no sleep, in any context that `call_rcu()` may be called in.
/// [`synchronize()`](Self::synchronize) is the blocking way to get the owner back.
pub struct Retired<P: RcuOwner, D: RcuDomain = Kernel> {
    ptr: NonNull<P::Target>,
    _owner: PhantomData<(P, D)>,
}

// SAFETY: (U6) a Retired is an owner that has left the pointer, and is not shared;
// moving it to another thread moves the owner, which is what P: Send (a supertrait of
// RcuOwner) allows.
unsafe impl<P: RcuOwner, D: RcuDomain> Send for Retired<P, D> {}

impl<P: RcuOwner, D: RcuDomain> Retired<P, D> {
    /// Waits for a grace period, `synchronize_rcu()`, and gives back the owner. No reader
    /// that could have seen the object is left. May sleep.
    ///
    /// # Safety
    ///
    /// The caller is not in a read-side critical section, and may sleep. On a
    /// configuration where `synchronize_rcu()` does not wait for the caller's own
    /// critical section, calling it in one hands the owner back, and so lets it be
    /// dropped, while a reference from `read()` is live.
    pub unsafe fn synchronize(self) -> P {
        let this = ManuallyDrop::new(self);

        // SAFETY: (U1) synchronize_rcu(): the caller's contract.
        unsafe { D::synchronize() };
        // SAFETY: (U4) `ptr` came from P::into_raw(), was unpublished, and a grace period
        // has elapsed; `this` is not dropped, so it is taken back once.
        unsafe { P::from_raw(this.ptr) }
    }
}

/// The head of the object that `ptr` points to.
fn head_of<T: HasRcuHead>(ptr: NonNull<T>) -> *mut RcuHead {
    ptr.as_ptr().wrapping_byte_add(T::OFFSET).cast()
}

/// What `call_rcu()` calls after the grace period: takes the owner back and drops it.
///
/// # Safety
///
/// `head` is the head of an object that a `Retired<P>` unpublished, and this is the one
/// call for it.
unsafe extern "C" fn reclaim<P: RcuOwner>(head: *mut RcuHead) {
    let obj = head
        .wrapping_byte_sub(<P::Target as HasRcuHead>::OFFSET)
        .cast::<P::Target>();

    if let Some(obj) = NonNull::new(obj) {
        // SAFETY: (U4) the caller's contract: the object was unpublished by a Retired, a
        // grace period has elapsed, and this is the one call, so the owner is taken back once.
        drop(unsafe { P::from_raw(obj) });
    }
}

impl<P: RcuOwner, D: RcuDomain> Drop for Retired<P, D> {
    fn drop(&mut self) {
        // SAFETY: (U1) the head is in the object that `ptr` points to, which the owner owns and
        // nothing else queues; reclaim() takes that owner back after the grace period, once.
        unsafe { D::call_rcu(head_of(self.ptr), reclaim::<P>) };
    }
}

/// `rcu_dereference()` on `cell`, under `_guard`. The load is `Acquire`, since Rust cannot
/// order by address dependency: C's `READ_ONCE()` is weaker on arm64.
pub(crate) fn read_in<'g, T, C: Atomic<T>, D: RcuDomain>(
    cell: &C,
    _guard: &'g RcuGuard<D>,
) -> Option<&'g T> {
    let raw = NonNull::new(cell.load(Ordering::Acquire))?;

    // SAFETY: (U5) `_guard` is a read-side critical section that outlives `'g`, and a writer
    // drops the object only after a grace period that waits for it: reclaim(), or
    // Retired::synchronize(), which is not called in one. The Acquire load orders what the
    // writer initialized.
    Some(unsafe { &*raw.as_ptr() })
}

/// `rcu_replace_pointer()` on `cell`: publishes `new` and retires the old owner. The swap is
/// `AcqRel`: `Release`, as `rcu_assign_pointer()`'s `smp_store_release()`, for what initialized
/// the new object; `Acquire` for what the previous publisher did to the old one, which is
/// dropped after the grace period.
pub(crate) fn replace_in<P: RcuOwner, D: RcuDomain, C: Atomic<P::Target>>(
    cell: &C,
    new: Option<P>,
) -> Option<Retired<P, D>> {
    let new = new.map_or(ptr::null_mut(), |owner| owner.into_raw().as_ptr());
    let old = cell.swap(new, Ordering::AcqRel);

    NonNull::new(old).map(|ptr| Retired {
        ptr,
        _owner: PhantomData,
    })
}

/// A pointer that readers follow under [`RcuGuard`], for a word that someone else updates:
/// the view of a C `struct bar __rcu *ptr` in a structure C owns.
///
/// It owns nothing and updates nothing. The layout is the pointer's: `repr(transparent)`
/// over an atomic pointer, which is what makes the view of a C word a cast.
#[repr(transparent)]
pub struct RcuReadPtr<T> {
    ptr: AtomicPtr<T>,
    _lends: PhantomData<*const T>,
}

// SAFETY: (U6) a shared RcuReadPtr lends `&T` to every thread that holds a guard, which is
// what T: Sync allows. Nothing else is shared.
unsafe impl<T: Sync> Sync for RcuReadPtr<T> {}

// SAFETY: (U6) moving the view moves a pointer that lends `&T` on whichever thread reads it.
unsafe impl<T: Sync> Send for RcuReadPtr<T> {}

kr::static_assert!(mem::size_of::<RcuReadPtr<()>>() == mem::size_of::<*mut ()>());
kr::static_assert!(mem::align_of::<RcuReadPtr<()>>() == mem::align_of::<*mut ()>());

impl<T> RcuReadPtr<T> {
    /// View a pointer that C declares `__rcu`, in a structure C owns, for reading.
    ///
    /// # Safety
    ///
    /// `ptr` points to an `__rcu` pointer word that stays where it is for `'a`, and is NULL
    /// or points to an object of type `T` that stays live while any reader can see it: its
    /// updaters replace it with `rcu_assign_pointer()` and free the old object only after a
    /// grace period.
    pub unsafe fn from_raw<'a>(ptr: *const *mut T) -> &'a Self {
        // SAFETY: (U2) RcuReadPtr is repr(transparent) over a pointer, which is the layout of
        // the word (asserted above); the caller's contract keeps it live for 'a.
        unsafe { &*ptr.cast::<Self>() }
    }

    /// `rcu_dereference()`: the object, for as long as `guard` lasts, or `None`.
    pub fn read<'g>(&self, guard: &'g RcuGuard) -> Option<&'g T> {
        read_in(&self.ptr, guard)
    }

    /// `rcu_access_pointer(p) != NULL`: whether something is published. Nothing is
    /// dereferenced, so no guard is needed, and the answer may be stale.
    pub fn is_some(&self) -> bool {
        !self.ptr.load(Ordering::Relaxed).is_null()
    }
}

/// A pointer that readers follow under [`RcuGuard`] and that a writer replaces, and that owns
/// the object it points to.
///
/// `P` is an [`RcuOwner`], such as `KRef<T>`, and the object embeds a [`RcuHead`]. The
/// pointer is NULL until something is published.
///
/// Updaters serialize themselves, as in C, with a lock of their own: two
/// [`replace()`](Self::replace)s that race are memory-safe (each old owner is returned
/// once), and which one wins is not defined. Dropping the `RcuPtr` retires what is
/// published, which is deferred and does not sleep.
///
/// The layout is the pointer's, for any owner: `repr(transparent)` over an [`RcuReadPtr`]
/// and a zero-sized marker, which rustc checks.
#[repr(transparent)]
pub struct RcuPtr<P: RcuOwner> {
    word: RcuReadPtr<P::Target>,
    _owner: PhantomData<fn() -> P>,
}

impl<P: RcuOwner> RcuPtr<P> {
    /// A pointer that points to nothing: `RCU_INIT_POINTER(p, NULL)`.
    pub const fn empty() -> Self {
        Self {
            word: RcuReadPtr {
                ptr: AtomicPtr::new(ptr::null_mut()),
                _lends: PhantomData,
            },
            _owner: PhantomData,
        }
    }

    /// A pointer that owns `owner`, not yet visible to any reader: `RCU_INIT_POINTER()`, so a
    /// plain store.
    pub fn new(owner: P) -> Self {
        Self {
            word: RcuReadPtr {
                ptr: AtomicPtr::new(owner.into_raw().as_ptr()),
                _lends: PhantomData,
            },
            _owner: PhantomData,
        }
    }

    /// View a pointer that C declares `__rcu`, in a structure C owns, as one that Rust
    /// updates and owns the object of.
    ///
    /// # Safety
    ///
    /// `ptr` points to an `__rcu` pointer word that stays where it is for `'a`, and is NULL or
    /// holds **one counted owner** that some `P` can take back: `replace()` and `take()` do,
    /// and drop it after a grace period. So **no C updater frees an object of this word** (with
    /// `kfree_rcu()`, or after `synchronize_rcu()`): it would free what an `RcuPtr` also drops.
    /// C may replace the pointer with `rcu_assign_pointer()` only by handing its old owner to a
    /// `P` of its own. For a word that C updates, use [`RcuReadPtr`].
    pub unsafe fn from_raw<'a>(ptr: *mut *mut P::Target) -> &'a Self {
        // SAFETY: (U2) RcuPtr is repr(transparent) over an RcuReadPtr, which is over a
        // pointer, the layout of the word; the caller's contract keeps it live for 'a.
        unsafe { &*ptr.cast::<Self>() }
    }

    /// `rcu_dereference()`: the object, for as long as `guard` lasts, or `None`.
    pub fn read<'g>(&self, guard: &'g RcuGuard) -> Option<&'g P::Target> {
        self.word.read(guard)
    }

    /// `rcu_access_pointer(p) != NULL`: whether something is published. Nothing is
    /// dereferenced, so no guard is needed, and the answer may be stale.
    pub fn is_some(&self) -> bool {
        self.word.is_some()
    }

    /// `rcu_replace_pointer()`: publishes `new` and returns the owner it replaced, retired.
    pub fn replace(&self, new: Option<P>) -> Option<Retired<P>> {
        replace_in(&self.word.ptr, new)
    }

    /// `rcu_assign_pointer()`: publishes `owner`, and returns the owner it replaced, retired.
    pub fn assign(&self, owner: P) -> Option<Retired<P>> {
        self.replace(Some(owner))
    }

    /// Unpublishes the object, and returns its owner, retired.
    pub fn take(&self) -> Option<Retired<P>> {
        self.replace(None)
    }

    /// `rcu_dereference_protected()`: the object, without a guard, for an updater.
    ///
    /// # Safety
    ///
    /// No updater runs while the reference lives: the caller holds the lock that serializes
    /// them, so nothing can replace the object, and so nothing can retire it, however long the
    /// reference is used.
    pub unsafe fn read_protected(&self) -> Option<&P::Target> {
        // Relaxed: as C's plain load. The update-side lock is what orders this load after the
        // last updater's store.
        let raw = NonNull::new(self.word.ptr.load(Ordering::Relaxed))?;

        // SAFETY: (U3) the caller's contract: the update-side lock is held, so no updater can
        // replace and retire the object while this reference lives.
        Some(unsafe { &*raw.as_ptr() })
    }
}

impl<P: RcuOwner> Default for RcuPtr<P> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<P: RcuOwner> Drop for RcuPtr<P> {
    /// Retires what is published, which is deferred: it does not sleep.
    fn drop(&mut self) {
        drop(self.take());
    }
}
