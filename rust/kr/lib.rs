// SPDX-License-Identifier: GPL-2.0

//! Kernel Rust core (`kr`).
//!
//! Zero-dependency primitives shared by in-kernel Rust ports and the
//! out-of-tree userspace test harness. Only `core` may be used: this crate
//! must build without the kernel, `bindings`, or any other in-tree crate.

#![cfg_attr(not(test), no_std)]

mod opaque;
mod static_assert;

pub use opaque::{Opaque, OpaqueBytes};
