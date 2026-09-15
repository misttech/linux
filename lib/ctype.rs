// SPDX-License-Identifier: GPL-2.0

//! linux/lib/ctype.c
//!
//! Copyright (C) 1991, 1992  Linus Torvalds
//!
//! The Rust implementation of `lib/ctype.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`: the `_ctype` table that the macros of
//! `include/linux/ctype.h` read. `lib/ctype_ffi.c` exports it with the
//! license of `lib/ctype.c`. C callers keep the header's macros; Rust callers
//! use the functions below, which are those macros on the same table.
//!
//! NOTE! This ctype does not handle EOF like the standard C library is
//! required to.

/// `_U`: upper.
pub const _U: u8 = 0x01;
/// `_L`: lower.
pub const _L: u8 = 0x02;
/// `_D`: digit.
pub const _D: u8 = 0x04;
/// `_C`: cntrl.
pub const _C: u8 = 0x08;
/// `_P`: punct.
pub const _P: u8 = 0x10;
/// `_S`: white space (space/lf/tab).
pub const _S: u8 = 0x20;
/// `_X`: hex digit.
pub const _X: u8 = 0x40;
/// `_SP`: hard space (0x20).
pub const _SP: u8 = 0x80;

#[rustfmt::skip]
const CTYPE: [u8; 256] = [
_C,_C,_C,_C,_C,_C,_C,_C,				/* 0-7 */
_C,_C|_S,_C|_S,_C|_S,_C|_S,_C|_S,_C,_C,			/* 8-15 */
_C,_C,_C,_C,_C,_C,_C,_C,				/* 16-23 */
_C,_C,_C,_C,_C,_C,_C,_C,				/* 24-31 */
_S|_SP,_P,_P,_P,_P,_P,_P,_P,				/* 32-39 */
_P,_P,_P,_P,_P,_P,_P,_P,				/* 40-47 */
_D,_D,_D,_D,_D,_D,_D,_D,				/* 48-55 */
_D,_D,_P,_P,_P,_P,_P,_P,				/* 56-63 */
_P,_U|_X,_U|_X,_U|_X,_U|_X,_U|_X,_U|_X,_U,		/* 64-71 */
_U,_U,_U,_U,_U,_U,_U,_U,				/* 72-79 */
_U,_U,_U,_U,_U,_U,_U,_U,				/* 80-87 */
_U,_U,_U,_P,_P,_P,_P,_P,				/* 88-95 */
_P,_L|_X,_L|_X,_L|_X,_L|_X,_L|_X,_L|_X,_L,		/* 96-103 */
_L,_L,_L,_L,_L,_L,_L,_L,				/* 104-111 */
_L,_L,_L,_L,_L,_L,_L,_L,				/* 112-119 */
_L,_L,_L,_P,_P,_P,_P,_C,				/* 120-127 */
0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,			/* 128-143 */
0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,			/* 144-159 */
_S|_SP,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,	/* 160-175 */
_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,_P,	/* 176-191 */
_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,_U,	/* 192-207 */
_U,_U,_U,_U,_U,_U,_U,_P,_U,_U,_U,_U,_U,_U,_U,_L,	/* 208-223 */
_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,_L,	/* 224-239 */
_L,_L,_L,_L,_L,_L,_L,_P,_L,_L,_L,_L,_L,_L,_L,_L,	/* 240-255 */
];

// The header indexes the table with any unsigned char.
kr::static_assert!(CTYPE.len() == 256);

/// `_ctype`: the character class of each `unsigned char`, which the
/// `include/linux/ctype.h` macros read.
#[allow(non_upper_case_globals)]
#[no_mangle]
pub static _ctype: [u8; 256] = CTYPE;

/// `__ismask(c)`.
#[inline]
fn ismask(c: u8) -> u8 {
    _ctype[usize::from(c)]
}

/// `isalnum(c)`.
#[inline]
pub fn isalnum(c: u8) -> bool {
    (ismask(c) & (_U | _L | _D)) != 0
}

/// `isalpha(c)`.
#[inline]
pub fn isalpha(c: u8) -> bool {
    (ismask(c) & (_U | _L)) != 0
}

/// `iscntrl(c)`.
#[inline]
pub fn iscntrl(c: u8) -> bool {
    (ismask(c) & _C) != 0
}

/// `isgraph(c)`.
#[inline]
pub fn isgraph(c: u8) -> bool {
    (ismask(c) & (_P | _U | _L | _D)) != 0
}

/// `islower(c)`.
#[inline]
pub fn islower(c: u8) -> bool {
    (ismask(c) & _L) != 0
}

/// `isprint(c)`.
#[inline]
pub fn isprint(c: u8) -> bool {
    (ismask(c) & (_P | _U | _L | _D | _SP)) != 0
}

/// `ispunct(c)`.
#[inline]
pub fn ispunct(c: u8) -> bool {
    (ismask(c) & _P) != 0
}

/// `isspace(c)`.
///
/// Note: isspace() must return false for %NUL-terminator
#[inline]
pub fn isspace(c: u8) -> bool {
    (ismask(c) & _S) != 0
}

/// `isupper(c)`.
#[inline]
pub fn isupper(c: u8) -> bool {
    (ismask(c) & _U) != 0
}

/// `isxdigit(c)`.
#[inline]
pub fn isxdigit(c: u8) -> bool {
    (ismask(c) & (_D | _X)) != 0
}

/// `isascii(c)`.
#[inline]
pub fn isascii(c: u8) -> bool {
    c <= 0x7f
}

/// `toascii(c)`.
#[inline]
pub fn toascii(c: u8) -> u8 {
    c & 0x7f
}

/// `isdigit(c)`.
#[inline]
pub fn isdigit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// `tolower(c)`, which is `__tolower(c)`.
#[inline]
pub fn tolower(c: u8) -> u8 {
    if isupper(c) {
        c.wrapping_add(b'a' - b'A')
    } else {
        c
    }
}

/// `toupper(c)`, which is `__toupper(c)`.
#[inline]
pub fn toupper(c: u8) -> u8 {
    if islower(c) {
        c.wrapping_sub(b'a' - b'A')
    } else {
        c
    }
}

/// `_tolower(c)`.
///
/// Fast implementation of tolower() for internal usage. Do not use in your
/// code.
#[inline]
pub fn _tolower(c: u8) -> u8 {
    c | 0x20
}

/// `isodigit(c)`: fast check for octal digit.
#[inline]
pub fn isodigit(c: u8) -> bool {
    (b'0'..=b'7').contains(&c)
}
