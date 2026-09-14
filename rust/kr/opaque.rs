// SPDX-License-Identifier: GPL-2.0

//! Storage for C data that Rust must not interpret.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

/// C data opaque to Rust: possibly uninitialized and mutable from C at any
/// time.
///
/// Use for fields a mirror carries but does not port, including fields whose
/// layout depends on kernel config.
#[repr(transparent)]
pub struct Opaque<T>(MaybeUninit<UnsafeCell<T>>);

impl<T> Opaque<T> {
    /// Creates an initialized `Opaque`.
    pub const fn new(value: T) -> Self {
        Self(MaybeUninit::new(UnsafeCell::new(value)))
    }

    /// Creates an uninitialized `Opaque`.
    pub const fn uninit() -> Self {
        Self(MaybeUninit::uninit())
    }

    /// Returns a raw pointer to the contained data.
    pub const fn get(&self) -> *mut T {
        UnsafeCell::raw_get(self.0.as_ptr())
    }
}

/// `SIZE` bytes of opaque C storage.
///
/// Wrap in a `#[repr(C, align(N))]` newtype when the C type needs alignment
/// greater than 1.
pub type OpaqueBytes<const SIZE: usize> = Opaque<[u8; SIZE]>;
