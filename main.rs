// SPDX-License-Identifier: GPL-2.0

//! Rust implementations of ported core kernel units.
//!
//! Built only with `CONFIG_RUST_KERNEL`. Each port sits next to the C file it
//! replaces and is added here as one `#[path]` module.

#![no_std]

#[path = "lib/refcount.rs"]
pub mod refcount;
