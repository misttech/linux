// SPDX-License-Identifier: GPL-2.0

//! Windowed min/max tracker
//!
//! Kathleen Nichols' algorithm for tracking the minimum (or maximum)
//! value of a data stream over some fixed time interval.  (E.g.,
//! the minimum RTT over the past five minutes.) It uses constant
//! space and constant time per update yet almost always delivers
//! the same minimum as an implementation that has to keep all the
//! data in the window.
//!
//! The algorithm keeps track of the best, 2nd best & 3rd best min
//! values, maintaining an invariant that the measurement time of
//! the n'th best >= n-1'th best. It also makes sure that the three
//! values are widely separated in the time window since that bounds
//! the worse case error when that data is monotonically increasing
//! over the window.
//!
//! Upon getting a new min, we can forget everything earlier because
//! it has no value - the new min is <= everything else in the window
//! by definition and it's the most recent. So we restart fresh on
//! every new min and overwrites 2nd & 3rd choices. The same property
//! holds for 2nd & 3rd best.
//!
//! The Rust implementation of `lib/win_minmax.c`, built instead of it with
//! `CONFIG_RUST_KERNEL`. `include/linux/win_minmax.h` is the ABI contract,
//! and its static inlines stay in C. `lib/win_minmax_ffi.c` exports these
//! symbols with the license of `lib/win_minmax.c`.

/// Mirror of `struct minmax_sample` (`include/linux/win_minmax.h`).
#[allow(non_camel_case_types)]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct minmax_sample {
    /// Time the measurement was taken.
    t: u32,
    /// Value measured.
    v: u32,
}

/// Mirror of `struct minmax`.
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct minmax {
    s: [minmax_sample; 3],
}

// u32 fields only: the layout does not depend on pointer width or config.
// Values from pahole of BTF of the running distro kernel 7.0.0-30-generic,
// matching a userspace compile of the same header.
kr::static_assert_layout!(minmax_sample, size = 8, align = 4, t @ 0, v @ 4);
kr::static_assert_layout!(minmax, size = 24, align = 4, s @ 0);

/// `minmax_reset()` from the header: all three samples become `{t, meas}`.
fn minmax_reset(m: &mut minmax, t: u32, meas: u32) -> u32 {
    let val = minmax_sample { t, v: meas };

    m.s[2] = val;
    m.s[1] = val;
    m.s[0] = val;
    m.s[0].v
}

/// As time advances, update the 1st, 2nd, and 3rd choices.
fn minmax_subwin_update(m: &mut minmax, win: u32, val: &minmax_sample) -> u32 {
    // C unsigned subtraction wraps; `t` is a jiffies-style clock.
    let dt = val.t.wrapping_sub(m.s[0].t);

    if dt > win {
        /*
         * Passed entire window without a new val so make 2nd
         * choice the new val & 3rd choice the new 2nd choice.
         * we may have to iterate this since our 2nd choice
         * may also be outside the window (we checked on entry
         * that the third choice was in the window).
         */
        m.s[0] = m.s[1];
        m.s[1] = m.s[2];
        m.s[2] = *val;
        if val.t.wrapping_sub(m.s[0].t) > win {
            m.s[0] = m.s[1];
            m.s[1] = m.s[2];
            m.s[2] = *val;
        }
    } else if m.s[1].t == m.s[0].t && dt > win / 4 {
        /*
         * We've passed a quarter of the window without a new val
         * so take a 2nd choice from the 2nd quarter of the window.
         */
        m.s[1] = *val;
        m.s[2] = *val;
    } else if m.s[2].t == m.s[1].t && dt > win / 2 {
        /*
         * We've passed half the window without finding a new val
         * so take a 3rd choice from the last half of the window
         */
        m.s[2] = *val;
    }
    m.s[0].v
}

/// Check if new measurement updates the 1st, 2nd or 3rd choice max.
///
/// # Safety
///
/// `m` points to a live `struct minmax` for the duration of the call. The
/// caller serializes updates of this tracker, as C callers do.
#[no_mangle]
pub unsafe extern "C" fn minmax_running_max(m: *mut minmax, win: u32, t: u32, meas: u32) -> u32 {
    // SAFETY: (U3) the C contract of minmax_running_max(): m is the
    // caller's tracker, live for this call.
    let m = unsafe { &mut *m };
    let val = minmax_sample { t, v: meas };

    if val.v >= m.s[0].v || val.t.wrapping_sub(m.s[2].t) > win {
        /* found new max? / nothing left in window? */
        return minmax_reset(m, t, meas); /* forget earlier samples */
    }

    if val.v >= m.s[1].v {
        m.s[1] = val;
        m.s[2] = val;
    } else if val.v >= m.s[2].v {
        m.s[2] = val;
    }

    minmax_subwin_update(m, win, &val)
}

/// Check if new measurement updates the 1st, 2nd or 3rd choice min.
///
/// # Safety
///
/// As [`minmax_running_max()`].
#[no_mangle]
pub unsafe extern "C" fn minmax_running_min(m: *mut minmax, win: u32, t: u32, meas: u32) -> u32 {
    // SAFETY: (U3) the C contract of minmax_running_min(): m is the
    // caller's tracker, live for this call.
    let m = unsafe { &mut *m };
    let val = minmax_sample { t, v: meas };

    if val.v <= m.s[0].v || val.t.wrapping_sub(m.s[2].t) > win {
        /* found new min? / nothing left in window? */
        return minmax_reset(m, t, meas); /* forget earlier samples */
    }

    if val.v <= m.s[1].v {
        m.s[1] = val;
        m.s[2] = val;
    } else if val.v <= m.s[2].v {
        m.s[2] = val;
    }

    minmax_subwin_update(m, win, &val)
}
