// SPDX-License-Identifier: GPL-2.0

//! Unified UUID/GUID definition, replacing `lib/uuid.c` in place.
//!
//! Copyright (C) 2009, 2016 Intel Corp.
//!    Huang Ying <ying.huang@intel.com>
//!
//! The two null values and the two byte-order tables are data symbols, so the
//! port defines them as statics with the C names. The functions call
//! `get_random_bytes()` and `hex_to_bin()`, which are ordinary exported C
//! functions, and `isxdigit()` is `u8::is_ascii_hexdigit()`: the `_ctype` table
//! marks exactly `0-9`, `a-f` and `A-F`, which the harness checks for all 256
//! byte values.

use core::ffi::{c_char, c_int, c_uchar, c_void};

/// `guid_t`, `typedef struct { __u8 b[16]; } guid_t`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct guid_t {
    b: [u8; 16],
}

/// `uuid_t`, `typedef struct { __u8 b[16]; } uuid_t`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct uuid_t {
    b: [u8; 16],
}

// BTF of x86_64 kernels: one array of 16 bytes each, in every config.
kr::static_assert_layout!(guid_t, size = 16, align = 1, b @ 0);
kr::static_assert_layout!(uuid_t, size = 16, align = 1, b @ 0);

/// `UUID_STRING_LEN`, the length of `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
const UUID_STRING_LEN: usize = 36;

unsafe extern "C" {
    /// `get_random_bytes()`.
    fn get_random_bytes(buf: *mut c_void, len: usize);
    /// `hex_to_bin()`: the value of a hex digit, or -1.
    fn hex_to_bin(ch: c_uchar) -> c_int;
}

/// `const guid_t guid_null`, all zeroes.
#[unsafe(no_mangle)]
pub static guid_null: guid_t = guid_t { b: [0; 16] };

/// `const uuid_t uuid_null`, all zeroes.
#[unsafe(no_mangle)]
pub static uuid_null: uuid_t = uuid_t { b: [0; 16] };

/// `const u8 guid_index[16]`: where each byte of the string goes in a GUID. Not
/// exported, but `lib/vsprintf.c` links it.
#[unsafe(no_mangle)]
pub static guid_index: [u8; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];

/// `const u8 uuid_index[16]`: where each byte of the string goes in a UUID. Not
/// exported, but `lib/vsprintf.c` links it.
#[unsafe(no_mangle)]
pub static uuid_index: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Fills `b` with random bytes.
fn random_bytes(b: &mut [u8; 16]) {
    // SAFETY: (U1) get_random_bytes() writes 16 bytes to a buffer of 16 bytes.
    unsafe { get_random_bytes(b.as_mut_ptr().cast(), b.len()) }
}

/// Reads the byte at `p`.
fn at(p: *const u8) -> u8 {
    // SAFETY: (U3) `p` is inside the NUL-terminated string that the caller gave
    // uuid_is_valid() or uuid_parse(). Callers read position `i` only after the `i`
    // before it were valid, none of which is NUL, so the read never passes the
    // terminator.
    unsafe { *p }
}

/// The 16 bytes at `p`, which the caller owns for the call.
fn bytes<'a>(p: *mut u8) -> &'a mut [u8; 16] {
    // SAFETY: (U3) the C contract of these functions: `p` points to 16 writable
    // bytes that nothing else accesses during the call.
    unsafe { &mut *p.cast::<[u8; 16]>() }
}

/// generate_random_uuid - generate a random UUID
/// @uuid: where to put the generated UUID
///
/// Random UUID interface
///
/// Used to create a Boot ID or a filesystem UUID/GUID, but can be
/// useful for other kernel drivers.
///
/// # Safety
///
/// `uuid` points to 16 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generate_random_uuid(uuid: *mut u8) {
    let uuid = bytes(uuid);

    random_bytes(uuid);
    /* Set UUID version to 4 --- truly random generation */
    uuid[6] = (uuid[6] & 0x0F) | 0x40;
    /* Set the UUID variant to DCE */
    uuid[8] = (uuid[8] & 0x3F) | 0x80;
}

/// # Safety
///
/// `guid` points to 16 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn generate_random_guid(guid: *mut u8) {
    let guid = bytes(guid);

    random_bytes(guid);
    /* Set GUID version to 4 --- truly random generation */
    guid[7] = (guid[7] & 0x0F) | 0x40;
    /* Set the GUID variant to DCE */
    guid[8] = (guid[8] & 0x3F) | 0x80;
}

fn uuid_gen_common(b: &mut [u8; 16]) {
    random_bytes(b);
    /* revision 0b10 */
    b[8] = (b[8] & 0x3F) | 0x80;
}

/// # Safety
///
/// `lu` points to a live `guid_t` that nothing else accesses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guid_gen(lu: *mut guid_t) {
    let lu = bytes(lu.cast());

    uuid_gen_common(lu);
    /* version 4 : random generation */
    lu[7] = (lu[7] & 0x0F) | 0x40;
}

/// # Safety
///
/// `bu` points to a live `uuid_t` that nothing else accesses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uuid_gen(bu: *mut uuid_t) {
    let bu = bytes(bu.cast());

    uuid_gen_common(bu);
    /* version 4 : random generation */
    bu[6] = (bu[6] & 0x0F) | 0x40;
}

/// uuid_is_valid - checks if a UUID string is valid
/// @uuid:    UUID string to check
///
/// Description:
/// It checks if the UUID string is following the format:
///    xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
///
/// where x is a hex digit.
///
/// Return: true if input is valid UUID string.
///
/// # Safety
///
/// `uuid` points to a NUL-terminated string, or to at least 36 readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uuid_is_valid(uuid: *const c_char) -> bool {
    is_valid_bytes(uuid.cast())
}

/// The check of uuid_is_valid(), which uuid_parse_common() makes too, for a `uuid` that
/// points to a NUL-terminated string, or to at least 36 readable bytes.
fn is_valid_bytes(uuid: *const u8) -> bool {
    for i in 0..UUID_STRING_LEN {
        let c = at(uuid.wrapping_add(i));

        if i == 8 || i == 13 || i == 18 || i == 23 {
            if c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }

    true
}

/// `-EINVAL`.
const EINVAL: c_int = 22;

fn uuid_parse_common(uuid: *const c_char, b: &mut [u8; 16], ei: &[u8; 16]) -> c_int {
    const SI: [u8; 16] = [0, 2, 4, 6, 9, 11, 14, 16, 19, 21, 24, 26, 28, 30, 32, 34];

    let uuid: *const u8 = uuid.cast();

    if !is_valid_bytes(uuid) {
        return -EINVAL;
    }

    for (i, &si) in SI.iter().enumerate() {
        let pos = usize::from(si);
        // SAFETY: (U1) hex_to_bin() takes one byte and reads nothing else.
        let hi = unsafe { hex_to_bin(at(uuid.wrapping_add(pos))) };
        // SAFETY: (U1) as above.
        let lo = unsafe { hex_to_bin(at(uuid.wrapping_add(pos + 1))) };

        b[usize::from(ei[i])] = ((hi << 4) | lo) as u8;
    }

    0
}

/// # Safety
///
/// `uuid` is as for [`uuid_is_valid()`], and `u` points to a live `guid_t` that
/// nothing else accesses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guid_parse(uuid: *const c_char, u: *mut guid_t) -> c_int {
    uuid_parse_common(uuid, bytes(u.cast()), &guid_index)
}

/// # Safety
///
/// `uuid` is as for [`uuid_is_valid()`], and `u` points to a live `uuid_t` that
/// nothing else accesses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uuid_parse(uuid: *const c_char, u: *mut uuid_t) -> c_int {
    uuid_parse_common(uuid, bytes(u.cast()), &uuid_index)
}
