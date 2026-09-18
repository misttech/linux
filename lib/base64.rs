// SPDX-License-Identifier: GPL-2.0
//! Base64 with support for multiple variants.
//!
//! This is the Rust replacement for `lib/base64.c`.

use core::ffi::{c_char, c_int};

const TABLES: [[u8; 64]; 3] = [
    *b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    *b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_",
    *b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,",
];
const REVERSE_CHARS: [(u8, u8); 3] = [(b'+', b'/'), (b'-', b'_'), (b'+', b',')];

fn reverse(value: u8, ch_62: u8, ch_63: u8) -> i32 {
    match value {
        b'A'..=b'Z' => i32::from(value - b'A'),
        b'a'..=b'z' => i32::from(value - b'a' + 26),
        b'0'..=b'9' => i32::from(value - b'0' + 52),
        _ if value == ch_62 => 62,
        _ if value == ch_63 => 63,
        _ => -1,
    }
}

/// Base64-encode a buffer using the requested variant.
///
/// # Safety
/// `src` must be valid for `srclen` bytes and `dst` must have room for the
/// encoded result. `variant` must name a `base64_variant` value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn base64_encode(
    src: *const u8,
    mut srclen: c_int,
    dst: *mut c_char,
    padding: bool,
    variant: c_int,
) -> c_int {
    // SAFETY: (U3) the C ABI requires valid source and destination ranges and
    // a valid base64_variant; all raw reads and writes stay within those ranges.
    unsafe {
        let mut src = src;
        let dst_start = dst.cast::<u8>();
        let mut dst = dst_start;
        let table = TABLES.get_unchecked(variant as usize);

        while srclen >= 3 {
            let ac =
                (u32::from(*src) << 16) | (u32::from(*src.add(1)) << 8) | u32::from(*src.add(2));
            *dst = table[(ac >> 18) as usize];
            *dst.add(1) = table[((ac >> 12) & 0x3f) as usize];
            *dst.add(2) = table[((ac >> 6) & 0x3f) as usize];
            *dst.add(3) = table[(ac & 0x3f) as usize];
            src = src.add(3);
            dst = dst.add(4);
            srclen -= 3;
        }

        match srclen {
            2 => {
                let ac = (u32::from(*src) << 16) | (u32::from(*src.add(1)) << 8);
                *dst = table[(ac >> 18) as usize];
                *dst.add(1) = table[((ac >> 12) & 0x3f) as usize];
                *dst.add(2) = table[((ac >> 6) & 0x3f) as usize];
                dst = if padding {
                    *dst.add(3) = b'=';
                    dst.add(4)
                } else {
                    dst.add(3)
                };
            }
            1 => {
                let ac = u32::from(*src) << 16;
                *dst = table[(ac >> 18) as usize];
                *dst.add(1) = table[((ac >> 12) & 0x3f) as usize];
                dst = if padding {
                    *dst.add(2) = b'=';
                    *dst.add(3) = b'=';
                    dst.add(4)
                } else {
                    dst.add(2)
                };
            }
            _ => {}
        }
        dst.offset_from(dst_start) as c_int
    }
}

/// Base64-decode a buffer using the requested variant.
///
/// # Safety
/// `src` must be valid for `srclen` bytes and `dst` must have room for the
/// decoded result. `variant` must name a `base64_variant` value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn base64_decode(
    src: *const c_char,
    mut srclen: c_int,
    dst: *mut u8,
    mut padding: bool,
    variant: c_int,
) -> c_int {
    // SAFETY: (U3) the C ABI requires valid source and destination ranges and
    // a valid base64_variant; all raw reads and writes stay within those ranges.
    unsafe {
        let mut src = src.cast::<u8>();
        let dst_start = dst;
        let mut dst = dst;
        let (ch_62, ch_63) = *REVERSE_CHARS.get_unchecked(variant as usize);

        while srclen >= 4 {
            let input0 = reverse(*src, ch_62, ch_63);
            let input1 = reverse(*src.add(1), ch_62, ch_63);
            let input2 = reverse(*src.add(2), ch_62, ch_63);
            let input3 = reverse(*src.add(3), ch_62, ch_63);
            let val = (input0 << 18) | (input1 << 12) | (input2 << 6) | input3;

            if val < 0 {
                if !padding || srclen != 4 || *src.add(3) != b'=' {
                    return -1;
                }
                padding = false;
                srclen = if *src.add(2) == b'=' { 2 } else { 3 };
                break;
            }

            *dst = (val >> 16) as u8;
            *dst.add(1) = (val >> 8) as u8;
            *dst.add(2) = val as u8;
            dst = dst.add(3);
            src = src.add(4);
            srclen -= 4;
        }

        if srclen == 0 {
            return dst.offset_from(dst_start) as c_int;
        }
        if padding || srclen == 1 {
            return -1;
        }

        let mut val =
            (reverse(*src, ch_62, ch_63) << 12) | (reverse(*src.add(1), ch_62, ch_63) << 6);
        if srclen == 2 {
            if val & 0x800003ff_u32 as i32 != 0 {
                return -1;
            }
            *dst = (val >> 10) as u8;
            dst = dst.add(1);
        } else {
            val |= reverse(*src.add(2), ch_62, ch_63);
            if val & 0x80000003_u32 as i32 != 0 {
                return -1;
            }
            *dst = (val >> 10) as u8;
            *dst.add(1) = (val >> 2) as u8;
            dst = dst.add(2);
        }
        dst.offset_from(dst_start) as c_int
    }
}
