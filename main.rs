// SPDX-License-Identifier: GPL-2.0

//! Rust implementations of ported core kernel units.
//!
//! Built only with `CONFIG_RUST_KERNEL`. Each port sits next to the C file it
//! replaces and is added here as one `#[path]` module.

#![no_std]

#[path = "lib/cmdline.rs"]
pub mod cmdline;

#[path = "lib/ctype.rs"]
pub mod ctype;

#[path = "lib/dec_and_lock.rs"]
pub mod dec_and_lock;

#[path = "lib/errseq.rs"]
pub mod errseq;

#[path = "lib/kref.rs"]
pub mod kref;

#[path = "lib/llist.rs"]
pub mod llist;

#[path = "lib/list_sort.rs"]
pub mod list_sort;

#[path = "lib/rcuref.rs"]
pub mod rcuref;

#[path = "lib/timerqueue.rs"]
pub mod timerqueue;

#[path = "lib/refcount.rs"]
pub mod refcount;

#[path = "lib/win_minmax.rs"]
pub mod win_minmax;
