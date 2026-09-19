// SPDX-License-Identifier: GPL-2.0
//! Binary-coded decimal conversion.
//!
//! This is the Rust replacement for `lib/bcd.c`.

use core::ffi::{c_uchar, c_uint};

/// Convert an eight-bit binary-coded decimal value to binary.
#[unsafe(no_mangle)]
pub extern "C" fn _bcd2bin(val: c_uchar) -> c_uint {
    c_uint::from(val & 0x0f) + c_uint::from(val >> 4) * 10
}

/// Convert an unsigned binary value to its eight-bit binary-coded decimal form.
#[unsafe(no_mangle)]
pub extern "C" fn _bin2bcd(val: c_uint) -> c_uchar {
    let tens = val.wrapping_mul(103) >> 10;

    (tens << 4 | val.wrapping_sub(tens * 10)) as c_uchar
}
