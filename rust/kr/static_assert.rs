// SPDX-License-Identifier: GPL-2.0

//! Compile-time assertions.

/// Static assert (i.e. compile-time assert).
///
/// Same form as `kernel::static_assert!`, usable without the `kernel` crate.
///
/// # Examples
///
/// ```
/// kr::static_assert!(core::mem::size_of::<u32>() == 4);
/// kr::static_assert!(core::mem::size_of::<u32>() == 4, "u32 must be 4 bytes");
/// ```
#[macro_export]
macro_rules! static_assert {
    ($condition:expr $(,$arg:literal)?) => {
        const _: () = ::core::assert!($condition $(,$arg)?);
    };
}

/// Asserts the exact layout of a `#[repr(C)]` mirror: size, alignment and the
/// offset of each listed field.
///
/// Values come from BTF (`pahole -C <type>`), never from hand calculation.
/// Size is checked for equality, not an upper bound: the mirror *is* the C
/// type.
///
/// # Examples
///
/// ```
/// #[repr(C)]
/// struct KrefMirror {
///     refcount: u32,
/// }
///
/// kr::static_assert_layout!(KrefMirror, size = 4, align = 4, refcount @ 0);
/// ```
#[macro_export]
macro_rules! static_assert_layout {
    ($ty:ty, size = $size:expr, align = $align:expr $(, $field:ident @ $offset:expr)* $(,)?) => {
        const _: () = ::core::assert!(
            ::core::mem::size_of::<$ty>() == $size,
            concat!("size mismatch: ", stringify!($ty))
        );
        const _: () = ::core::assert!(
            ::core::mem::align_of::<$ty>() == $align,
            concat!("align mismatch: ", stringify!($ty))
        );
        $(
            const _: () = ::core::assert!(
                ::core::mem::offset_of!($ty, $field) == $offset,
                concat!("offset mismatch: ", stringify!($ty), ".", stringify!($field))
            );
        )*
    };
}
