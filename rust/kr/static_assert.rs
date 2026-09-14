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
