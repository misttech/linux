// SPDX-License-Identifier: (GPL-2.0 OR MIT)

//! Shell-style pattern matching, replacing `lib/glob.c` in place.
//!
//! This is a small and simple implementation intended for device denylists
//! where a string is matched against a number of patterns. It does not
//! preprocess the patterns, is non-recursive, and its run-time is at most
//! quadratic.
//!
//! The functions read C strings through raw pointers and touch no other state,
//! so the whole port has one `unsafe` read. Pointers are advanced with wrapping
//! arithmetic, as the matcher steps one past the end of the string it is given.

use core::ffi::c_char;
use core::ptr;

/// Reads the byte at `p`.
#[inline]
fn at(p: *const u8) -> u8 {
    // SAFETY: (U3) `p` is inside the NUL-terminated pattern, or the string up to
    // its NUL or `str_end`. The matcher reads a position only after every byte
    // before it, and stops at the first NUL, or at `str_end` for a bounded string
    // without reading it.
    unsafe { *p }
}

/// glob_match - Shell-style pattern matching, like !fnmatch(pat, str, 0)
/// @pat: Shell-style pattern to match, e.g. "*.[ch]".
/// @str: String to match.  The pattern must match the entire string.
///
/// Perform shell-style glob matching, returning true (1) if the match
/// succeeds, or false (0) if it fails.  Equivalent to !fnmatch(@pat, @str, 0).
///
/// Pattern metacharacters are ?, *, [ and \.
/// (And, inside character classes, !, - and ].)
///
/// This is a small and simple implementation intended for device denylists
/// where a string is matched against a number of patterns.  Thus, it
/// does not preprocess the patterns.  It is non-recursive, and run-time
/// is at most quadratic: strlen(@str)*strlen(@pat).
///
/// An example of the worst case is glob_match("*aaaaa", "aaaaaaaaaa");
/// it takes 6 passes over the pattern before matching the string.
///
/// Like !fnmatch(@pat, @str, 0) and unlike the shell, this does NOT
/// treat / or leading . specially; it isn't actually used for pathnames.
///
/// Note that according to glob(7) (and unlike bash), character classes
/// are complemented by a leading !; this does not support the regex-style
/// [^a-z] syntax.
///
/// An opening bracket without a matching close is matched literally.
///
/// # Safety
///
/// `pat` and `s` point to NUL-terminated strings that live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glob_match(pat: *const c_char, s: *const c_char) -> bool {
    glob_match_str(pat.cast(), s.cast(), ptr::null())
}

/// glob_match_len - glob match against a length-bounded string
/// @pat: Shell-style pattern to match.
/// @str: String to match.  Need not be NUL-terminated.
/// @len: Number of bytes of @str that may be read.
///
/// Like glob_match(), but @str is only read up to @len bytes, so it can be
/// used on buffers that are not NUL-terminated (e.g. trace event fields).
/// A NUL byte within @len still terminates the string.
///
/// # Safety
///
/// `pat` points to a NUL-terminated string, and `s` to at least `len` readable
/// bytes or to a NUL-terminated string that is shorter, all live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glob_match_len(pat: *const c_char, s: *const c_char, len: usize) -> bool {
    let s: *const u8 = s.cast();

    glob_match_str(pat.cast(), s, s.wrapping_add(len))
}

fn glob_match_str(pat: *const u8, s: *const u8, str_end: *const u8) -> bool {
    let (mut pat, mut s) = (pat, s);
    /*
     * Backtrack to previous * on mismatch and retry starting one
     * character later in the string.  Because * matches all characters
     * (no exception for /), it can be easily proved that there's
     * never a need to backtrack multiple levels.
     */
    let mut back_pat: *const u8 = ptr::null();
    let mut back_str: *const u8 = ptr::null();

    /*
     * Loop over each token (character or class) in pat, matching
     * it against the remaining unmatched tail of str.  Return false
     * on mismatch, or true after matching the trailing nul bytes.
     */
    loop {
        let c = if !str_end.is_null() && s >= str_end {
            0
        } else {
            at(s)
        };
        let d = at(pat);
        pat = pat.wrapping_add(1);

        s = s.wrapping_add(1);

        // What the token leaves to do: compare `c` with a literal, or
        // backtrack to the last `*`.
        let mut literal = None;
        let mut backtrack = false;

        match d {
            /* Wildcard: anything but nul */
            b'?' => {
                if c == 0 {
                    return false;
                }
            }
            /* Any-length wildcard */
            b'*' => {
                /* Optimize trailing * case */
                if at(pat) == 0 {
                    return true;
                }
                back_pat = pat;
                /* Allow zero-length match */
                s = s.wrapping_sub(1);
                back_str = s;
            }
            /* Character class */
            b'[' => {
                /* No possible match */
                if c == 0 {
                    return false;
                }
                let mut matched = false;
                let inverted = at(pat) == b'!';
                let mut class = if inverted { pat.wrapping_add(1) } else { pat };
                let mut a = at(class);
                class = class.wrapping_add(1);
                let mut malformed = false;

                /*
                 * Iterate over each span in the character class.
                 * A span is either a single character a, or a
                 * range a-b.  The first span may begin with ']'.
                 */
                loop {
                    let mut b = a;

                    if a == 0 {
                        /* Malformed */
                        malformed = true;
                        break;
                    }

                    if at(class) == b'-' && at(class.wrapping_add(1)) != b']' {
                        b = at(class.wrapping_add(1));

                        if b == 0 {
                            malformed = true;
                            break;
                        }

                        class = class.wrapping_add(2);
                        /* Any special action if a > b? */
                    }
                    if a <= c && c <= b {
                        matched = true;
                    }

                    a = at(class);
                    class = class.wrapping_add(1);
                    if a == b']' {
                        break;
                    }
                }

                if malformed {
                    literal = Some(d);
                } else if matched == inverted {
                    backtrack = true;
                } else {
                    pat = class;
                }
            }
            b'\\' => {
                let d = at(pat);
                pat = pat.wrapping_add(1);
                literal = Some(d);
            }
            /* Literal character */
            _ => literal = Some(d),
        }

        if let Some(d) = literal {
            if c == d {
                if d == 0 {
                    return true;
                }
                continue;
            }
            backtrack = true;
        }
        if backtrack {
            /* No point continuing */
            if c == 0 || back_pat.is_null() {
                return false;
            }
            /* Try again from last *, one character later in str. */
            pat = back_pat;
            back_str = back_str.wrapping_add(1);
            s = back_str;
        }
    }
}
