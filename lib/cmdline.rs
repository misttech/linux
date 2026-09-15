// SPDX-License-Identifier: GPL-2.0-only

//! Helper functions generally used for parsing kernel command line and module
//! options.
//!
//! The Rust implementation of `lib/cmdline.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/string.h` declares these functions,
//! and `lib/cmdline_ffi.c` exports them with the license of `lib/cmdline.c`.
//!
//! Code and copyrights come from init/main.c and arch/i386/kernel/setup.c.

use core::ffi::{c_int, c_long, c_uint, c_ulonglong};
use core::ptr;
use core::slice;

/// The kernel's C `char`, which is always unsigned (`-funsigned-char`), unlike
/// `core::ffi::c_char` on most architectures.
#[allow(non_camel_case_types)]
type c_char = u8;

extern "C" {
    fn simple_strtoull(cp: *const c_char, endp: *mut *mut c_char, base: c_uint) -> c_ulonglong;
    fn simple_strtol(cp: *const c_char, endp: *mut *mut c_char, base: c_uint) -> c_long;
    fn skip_spaces(str: *const c_char) -> *mut c_char;
    fn strlen(s: *const c_char) -> usize;

    fn c_cmdline_isspace(c: c_char) -> bool;
}

/// The bytes of the NUL-terminated string at `str`, without the NUL.
///
/// # Safety
///
/// `str` is a NUL-terminated string that is not written for `'a`.
unsafe fn c_str_bytes<'a>(str: *const c_char) -> &'a [u8] {
    // SAFETY: (U1) strlen() reads the NUL-terminated string at str.
    let len = unsafe { strlen(str) };
    // SAFETY: (U3) the len bytes before the NUL are readable, and not written
    // for 'a.
    unsafe { slice::from_raw_parts(str, len) }
}

/// If a hyphen was found in get_option, this will handle the range of
/// numbers, M-N. This will expand the range and insert the values
/// `[M, M+1, ..., N]` into the ints array in get_options.
///
/// `pint` starts at the element holding M, and at most `n` values are stored.
///
/// # Safety
///
/// `*str` points to the hyphen of the range, inside a NUL-terminated string.
unsafe fn get_range(str: &mut *mut c_char, pint: &mut [c_int], n: c_int) -> c_int {
    *str = str.wrapping_add(1);
    // SAFETY: (U1) simple_strtol() reads the NUL-terminated string at *str,
    // which is inside the caller's string, past the hyphen.
    let upper_range = unsafe { simple_strtol(*str, ptr::null_mut(), 0) } as c_int;
    // get_option() stored M in the first element, which always exists.
    let Some(&lower_range) = pint.first() else {
        return 0;
    };
    let inc_counter = upper_range.wrapping_sub(lower_range);

    for (slot, x) in pint
        .iter_mut()
        .take(n.max(0) as usize)
        .zip(lower_range..upper_range)
    {
        *slot = x;
    }

    inc_counter
}

/// The body of [`get_option()`].
///
/// # Safety
///
/// `*str` is NULL or points into a NUL-terminated string.
unsafe fn parse_option(str: &mut *mut c_char, pint: Option<&mut c_int>) -> c_int {
    let mut cur = *str;

    if cur.is_null() {
        return 0;
    }
    // SAFETY: (U3) cur is not NULL, so it points into a NUL-terminated string.
    let first = unsafe { *cur };
    if first == 0 {
        return 0;
    }

    let value = if first == b'-' {
        cur = cur.wrapping_add(1);
        // SAFETY: (U1) simple_strtoull() reads the NUL-terminated string at
        // cur, past the hyphen, and stores where it stopped in *str.
        0u64.wrapping_sub(unsafe { simple_strtoull(cur, str, 0) }) as c_int
    } else {
        // SAFETY: (U1) simple_strtoull() reads the NUL-terminated string at
        // cur, and stores where it stopped in *str.
        (unsafe { simple_strtoull(cur, str, 0) }) as c_int
    };

    if let Some(pint) = pint {
        *pint = value;
    }
    if cur == *str {
        return 0;
    }

    // SAFETY: (U3) simple_strtoull() stopped inside the string, at most at its
    // NUL.
    match unsafe { **str } {
        b',' => {
            *str = str.wrapping_add(1);
            2
        }
        b'-' => 3,
        _ => 1,
    }
}

/// `get_option()`: parses an integer from an option string.
///
/// Read an int from an option string; if available accept a subsequent comma
/// as well.
///
/// When `pint` is NULL the function can be used as a validator of the current
/// option in the string.
///
/// Returns:
/// - 0: no int in string
/// - 1: int found, no subsequent comma
/// - 2: int found including a subsequent comma
/// - 3: hyphen found to denote a range
///
/// Leading hyphen without integer is no integer case, but we consume it for
/// the sake of simplification.
///
/// # Safety
///
/// `str` points to a string pointer that is NULL or points into a
/// NUL-terminated string, and `pint` is NULL or points to a writable int.
#[no_mangle]
pub unsafe extern "C" fn get_option(str: *mut *mut c_char, pint: *mut c_int) -> c_int {
    // SAFETY: (U3) the caller passes a valid pointer to its string pointer and
    // a NULL or writable pint, as the kernel-doc of get_option() requires.
    let (str, pint) = unsafe { (&mut *str, pint.as_mut()) };

    // SAFETY: (U3) *str is NULL or points into a NUL-terminated string.
    unsafe { parse_option(str, pint) }
}

/// `get_options()`: parses a string into a list of integers.
///
/// This function parses a string containing a comma-separated list of
/// integers, a hyphen-separated range of _positive_ integers, or a combination
/// of both. The parse halts when the array is full, or when no more numbers can
/// be retrieved from the string.
///
/// When `nints` is 0, the function just validates the given `str` and returns
/// the amount of parseable integers as described below.
///
/// The first element is filled by the number of collected integers in the
/// range. The rest is what was parsed from the `str`.
///
/// Returns the character in the string which caused the parse to end
/// (typically a null terminator, if `str` is completely parseable).
///
/// Where the C code would compute an index outside of `ints` (a range that
/// makes the count wrap around), this implementation stops the parse
/// instead of writing out of bounds.
///
/// # Safety
///
/// `str` is a NUL-terminated string, and `ints` has room for `nints`
/// integers, and for at least one.
#[no_mangle]
pub unsafe extern "C" fn get_options(
    str: *const c_char,
    nints: c_int,
    ints: *mut c_int,
) -> *mut c_char {
    let validate = nints == 0;
    // SAFETY: (U3) the caller passes ints with room for nints integers, and
    // for at least one.
    let ints = unsafe { slice::from_raw_parts_mut(ints, nints.max(1) as usize) };
    let mut str = str.cast_mut();
    let mut i: c_int = 1;

    while i < nints || validate {
        let at = if validate { 0 } else { i as usize };
        let Some(pint) = ints.get_mut(at..).filter(|pint| !pint.is_empty()) else {
            break;
        };

        // SAFETY: (U3) str points into the caller's NUL-terminated string.
        let res = unsafe { parse_option(&mut str, pint.first_mut()) };
        if res == 0 {
            break;
        }
        if res == 3 {
            let n = if validate { 0 } else { nints.wrapping_sub(i) };

            // SAFETY: (U3) parse_option() returned 3, so str points to the
            // hyphen of a range inside the caller's string.
            let range_nums = unsafe { get_range(&mut str, pint, n) };
            if range_nums < 0 {
                break;
            }
            // Decrement the result by one to leave out the
            // last number in the range.  The next iteration
            // will handle the upper number in the range
            i = i.wrapping_add(range_nums - 1);
        }
        i = i.wrapping_add(1);
        if res == 1 {
            break;
        }
    }

    if let Some(count) = ints.first_mut() {
        *count = i.wrapping_sub(1);
    }
    str
}

/// `memparse()`: parses a string with mem suffixes into a number.
///
/// Parses a string into a number. The number stored at `ptr` is potentially
/// suffixed with K, M, G, T, P, E.
///
/// Returns the value as recognized by `simple_strtoull()` multiplied by the
/// value as specified by suffix, if any.
///
/// # Safety
///
/// `ptr` is a NUL-terminated string, and `retptr` is NULL or points to a
/// writable string pointer.
#[no_mangle]
pub unsafe extern "C" fn memparse(ptr: *const c_char, retptr: *mut *mut c_char) -> c_ulonglong {
    // local pointer to end of parsed string
    let mut endptr: *mut c_char = ptr::null_mut();
    // SAFETY: (U1) simple_strtoull() reads the NUL-terminated string at ptr,
    // and stores where it stopped in endptr.
    let mut ret = unsafe { simple_strtoull(ptr, &mut endptr, 0) };

    // Consume valid suffix even in case of overflow.
    // SAFETY: (U3) simple_strtoull() stopped inside the string, at most at its
    // NUL.
    let shl: u32 = match unsafe { *endptr } {
        b'E' | b'e' => 60,
        b'P' | b'p' => 50,
        b'T' | b't' => 40,
        b'G' | b'g' => 30,
        b'M' | b'm' => 20,
        b'K' | b'k' => 10,
        _ => 0,
    };

    if shl != 0 && ptr != endptr.cast_const() {
        // Have valid suffix with preceding number.
        // check_shl_overflow(): the shift is at most 60 bits.
        let shifted = ret << shl;
        ret = if shifted >> shl != ret {
            u64::MAX
        } else {
            shifted
        };
        endptr = endptr.wrapping_add(1);
    }

    // SAFETY: (U3) retptr is NULL or points to the caller's string pointer.
    if let Some(retptr) = unsafe { retptr.as_mut() } {
        *retptr = endptr;
    }

    ret
}

/// `parse_option_str()`: parses a string and checks whether an option is set.
///
/// This function parses a string containing a comma-separated list of strings
/// like a=b,c.
///
/// Returns true if there's such option in the string, false otherwise.
///
/// # Safety
///
/// `str` and `option` are NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn parse_option_str(str: *const c_char, option: *const c_char) -> bool {
    // SAFETY: (U3) str and option are NUL-terminated strings, which are not
    // written during this call.
    let (mut str, option) = unsafe { (c_str_bytes(str), c_str_bytes(option)) };

    while !str.is_empty() {
        if let Some(rest) = str.strip_prefix(option) {
            str = rest;
            if matches!(str.first(), None | Some(b',')) {
                return true;
            }
        }

        let end = str.iter().position(|&c| c == b',').unwrap_or(str.len());
        str = str.get(end..).unwrap_or_default();

        if let Some(rest) = str.strip_prefix(b",") {
            str = rest;
        }
    }

    false
}

/// Clears the closing quote just before `end`, if there is one.
fn clear_closing_quote(buf: &mut [u8], end: usize) {
    if let Some(c) = end.checked_sub(1).and_then(|last| buf.get_mut(last)) {
        if *c == b'"' {
            *c = 0;
        }
    }
}

/// `next_arg()`: parses a string to get a param value pair.
///
/// You can use " around spaces, but can't escape ". Hyphens and underscores
/// equivalent in parameter names.
///
/// # Safety
///
/// `args` is a writable NUL-terminated string that nothing else accesses during
/// the call, and `param` and `val` point to writable string pointers.
#[no_mangle]
pub unsafe extern "C" fn next_arg(
    args: *mut c_char,
    param: *mut *mut c_char,
    val: *mut *mut c_char,
) -> *mut c_char {
    let mut args = args;
    let mut in_quote = false;
    let mut quoted = false;

    // SAFETY: (U3) args points to a NUL-terminated string.
    if unsafe { *args } == b'"' {
        args = args.wrapping_add(1);
        in_quote = true;
        quoted = true;
    }

    let mut i = 0;
    let mut equals = 0;
    loop {
        // SAFETY: (U3) the bytes before args[i] are not NUL, so args[i] is still
        // inside the NUL-terminated string, at most at its NUL.
        let c = unsafe { *args.wrapping_add(i) };
        if c == 0 {
            break;
        }
        // SAFETY: (U1) c_cmdline_isspace() is isspace(), a lookup in the
        // _ctype table.
        if unsafe { c_cmdline_isspace(c) } && !in_quote {
            break;
        }
        if equals == 0 && c == b'=' {
            equals = i;
        }
        if c == b'"' {
            in_quote = !in_quote;
        }
        i = i.wrapping_add(1);
    }

    // SAFETY: (U3) args[0..=i] are the scanned bytes and the one that stopped
    // the scan, all inside the writable string, which nothing else accesses
    // during this call.
    let buf = unsafe { slice::from_raw_parts_mut(args, i.wrapping_add(1)) };
    // SAFETY: (U3) param and val point to the caller's writable string
    // pointers, which are not part of the string.
    let (param, val) = unsafe { (&mut *param, &mut *val) };

    *param = args;
    if equals == 0 {
        *val = ptr::null_mut();
    } else {
        if let Some(c) = buf.get_mut(equals) {
            *c = 0;
        }
        *val = args.wrapping_add(equals.wrapping_add(1));

        // Don't include quotes in value.
        if buf.get(equals.wrapping_add(1)) == Some(&b'"') {
            *val = val.wrapping_add(1);
            clear_closing_quote(buf, i);
        }
    }
    if quoted {
        clear_closing_quote(buf, i);
    }

    let next = match buf.get_mut(i) {
        Some(c) if *c != 0 => {
            *c = 0;
            i.wrapping_add(1)
        }
        _ => i,
    };

    // Chew up trailing spaces.
    // SAFETY: (U1) skip_spaces() reads the NUL-terminated string at args +
    // next, which is inside the caller's string, at most at its NUL.
    unsafe { skip_spaces(args.wrapping_add(next)) }
}
