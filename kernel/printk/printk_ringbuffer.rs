// SPDX-License-Identifier: GPL-2.0-only
//! In-place printk ring buffer replacement.
//!
//! Shared C storage is accessed through the LKMM forwarding boundary.
//! Descriptor validation governs whether speculative snapshots may be used.

// Descriptor IDs are unsigned long; sequence numbers are always u64.
// Keep the widening conversions explicit for 32-bit kernels.
#![allow(clippy::unnecessary_cast)]

use core::ffi::{c_char, c_long, c_uint, c_ulong};
use core::ffi::{c_int, c_void};
use core::mem::{size_of, MaybeUninit};
use core::ptr::{addr_of, addr_of_mut, null_mut};

unsafe extern "C" {
    fn c_prb_load(p: *const c_long) -> c_long;
    fn c_prb_load_acquire(p: *const c_long) -> c_long;
    fn c_prb_store(p: *mut c_long, value: c_long);
    fn c_prb_cas(p: *mut c_long, old: *mut c_long, value: c_long) -> bool;
    fn c_prb_cas_relaxed(p: *mut c_long, old: *mut c_long, value: c_long) -> bool;
    fn c_prb_cas_release(p: *mut c_long, old: *mut c_long, value: c_long) -> bool;
    fn c_prb_inc(p: *mut c_long);
    fn c_prb_rmb();
    fn c_prb_irq_save(flags: *mut c_ulong);
    fn c_prb_irq_restore(flags: c_ulong);
    fn c_prb_copy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn c_prb_clear(dst: *mut c_void, value: c_int, n: usize) -> *mut c_void;
    fn c_prb_find(src: *const c_void, value: c_int, n: usize) -> *mut c_void;
    fn c_prb_panic_cpu() -> bool;
    fn c_prb_warn_0(condition: bool) -> bool;
    fn c_prb_warn_1(condition: bool) -> bool;
    fn c_prb_warn_2(condition: bool) -> bool;
    fn c_prb_warn_3(condition: bool) -> bool;
    fn c_prb_warn_4(condition: bool) -> bool;
    fn c_prb_warn_5(condition: bool) -> bool;
    fn c_prb_warn_6(condition: bool) -> bool;
    fn c_prb_warn_7(condition: bool) -> bool;
    fn c_prb_warn_8(condition: bool) -> bool;
    fn c_prb_warn_9(condition: bool) -> bool;
    fn c_prb_warn_10(condition: bool) -> bool;
    fn c_prb_warn_11(condition: bool) -> bool;
    fn c_prb_warn_len_zero(length: u16);
    fn c_prb_warn_len_max(length: u16, max: c_uint);
    static debug_non_panic_cpus: bool;
    static legacy_allow_panic_sync: bool;
}

const RESERVED: i32 = 0;
const EINVAL: c_int = -22;
const ENOENT: c_int = -2;

/// # Safety
/// ring's arrays are valid; desc is private writable storage.
unsafe fn finalized_seq(ring: *mut DescRing, id: c_ulong, seq: u64, desc: &mut Desc) -> c_int {
    let mut actual = 0;
    // SAFETY: (U1) snapshot access and state validation use the C boundary.
    let s = unsafe { desc_read(ring, id, desc, &mut actual, null_mut()) };
    if s == MISS || s == RESERVED || s == COMMITTED || actual != seq {
        return EINVAL;
    }
    if s == REUSABLE
        || (desc.text_blk_lpos.begin == FAILED_LPOS && desc.text_blk_lpos.next == FAILED_LPOS)
    {
        return ENOENT;
    }
    0
}

/// # Safety
/// text points to a C ring data range; its contents are speculative.
unsafe fn count_lines(text: *const c_char, size: c_uint) -> c_uint {
    // SAFETY: (U1) memchr performs the original C speculative text access.
    unsafe {
        let mut remaining = size as usize;
        let mut next = text;
        let mut count = 1;
        while remaining != 0 {
            let found = c_prb_find(next.cast(), 10, remaining).cast::<c_char>();
            if found.is_null() {
                break;
            }
            count += 1;
            let consumed = found.offset_from(next) as usize + 1;
            remaining -= consumed;
            next = found.add(1);
        }
        count
    }
}

/// # Safety
/// ring is valid, lpos is a descriptor snapshot, output buffers are caller-owned.
unsafe fn copy_data(
    ring: *const DataRing,
    lpos: &DataBlkLpos,
    len: u16,
    buf: *mut c_char,
    size: c_uint,
    lines: *mut c_uint,
) -> bool {
    if (buf.is_null() || size == 0) && lines.is_null() {
        return true;
    }
    // SAFETY: (U1) the C access layer copies only the range validated by get_data.
    unsafe {
        let mut available = 0;
        let data = get_data(ring, lpos, &mut available);
        if data.is_null() || available < c_uint::from(len) {
            return false;
        }
        if !lines.is_null() {
            *lines = count_lines(data, c_uint::from(len));
        }
        if !buf.is_null() && size != 0 {
            c_prb_copy(
                buf.cast(),
                data.cast(),
                size.min(c_uint::from(len)) as usize,
            );
        } // LMM(copy_data:A)
        true
    }
}

/// # Safety
/// rb is initialized; optional outputs are writable and do not alias shared data.
unsafe fn read_record(rb: *mut Ringbuffer, seq: u64, r: *mut Record, lines: *mut c_uint) -> c_int {
    // SAFETY: (U1) speculative C copies are bracketed by descriptor validation.
    unsafe {
        let ring = addr_of_mut!((*rb).desc_ring);
        let info = to_info(ring, seq);
        let d = to_desc(ring, seq);
        let id = load(addr_of!((*d).state_var)) & ID_MASK;
        let mut desc = empty_desc();
        let err = finalized_seq(ring, id, seq, &mut desc);
        if err != 0 || r.is_null() {
            return err;
        }
        if !(*r).info.is_null() {
            c_prb_copy((*r).info.cast(), info.cast(), size_of::<PrintkInfo>());
        }
        if !copy_data(
            addr_of!((*rb).text_data_ring),
            &desc.text_blk_lpos,
            read(addr_of!((*info).text_len)),
            (*r).text_buf,
            (*r).text_buf_size,
            lines,
        ) {
            return ENOENT;
        }
        finalized_seq(ring, id, seq, &mut desc)
    }
}

/// Find the sequence number at the current tail, including a data-less record.
/// # Safety
/// rb and all its arrays must remain alive and initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_first_seq(rb: *mut Ringbuffer) -> u64 {
    // SAFETY: (U1) the original C tail/descriptor barriers are preserved.
    unsafe {
        let ring = addr_of_mut!((*rb).desc_ring);
        loop {
            let id = load(addr_of!((*ring).tail_id));
            let mut seq = 0;
            let mut desc = empty_desc();
            let s = desc_read(ring, id, &mut desc, &mut seq, null_mut());
            if s == FINALIZED || s == REUSABLE {
                return seq;
            }
            // Guarantee the last state load from desc_read() is before
            // reloading @tail_id in order to see a new tail in the case
            // that the descriptor has been recycled. This pairs with
            // desc_reserve:D.
            //
            // Memory barrier involvement:
            //
            // If prb_first_seq:B reads from desc_reserve:F, then
            // prb_first_seq:A reads from desc_push_tail:B.
            //
            // Relies on:
            //
            // MB from desc_push_tail:B to desc_reserve:F
            //    matching
            // RMB from prb_first_seq:B to prb_first_seq:A
            c_prb_rmb(); // LMM(prb_first_seq:C)
        }
    }
}

/// Get the sequence number that the next reservation will receive.
/// # Safety
/// rb and all its arrays must remain alive and initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_next_reserve_seq(rb: *mut Ringbuffer) -> u64 {
    // SAFETY: (U1) sequence snapshots use the C atomic publication protocol.
    unsafe {
        let ring = addr_of_mut!((*rb).desc_ring);
        loop {
            let seq = last_finalized(rb);
            let head = load(addr_of!((*ring).head_id));
            let d = to_desc(ring, seq);
            let mut id = load(addr_of!((*d).state_var)) & ID_MASK;
            let mut desc = empty_desc();
            if finalized_seq(ring, id, seq, &mut desc) == EINVAL {
                if seq != 0 {
                    continue;
                }
                let initial = desc_count(ring).wrapping_add(1).wrapping_neg() & ID_MASK;
                if head == initial {
                    return 0;
                }
                id = initial.wrapping_add(1);
            }
            return seq
                .wrapping_add(head.wrapping_sub(id) as u64)
                .wrapping_add(1);
        }
    }
}

/// # Safety
/// rb is valid and seq and optional outputs belong to the reader.
unsafe fn read_valid(
    rb: *mut Ringbuffer,
    seq: &mut u64,
    r: *mut Record,
    lines: *mut c_uint,
) -> bool {
    // SAFETY: (U1) record snapshots and panic-state accesses use C operations.
    unsafe {
        loop {
            let err = read_record(rb, *seq, r, lines);
            if err == 0 {
                return true;
            }
            let tail = prb_first_seq(rb);
            if *seq < tail {
                *seq = tail;
            } else if err == ENOENT
                || (c_prb_panic_cpu()
                    && (read(addr_of!(debug_non_panic_cpus).cast::<u8>()) == 0
                        || read(addr_of!(legacy_allow_panic_sync).cast::<u8>()) != 0)
                    && seq.wrapping_add(1) < prb_next_reserve_seq(rb))
            {
                *seq = seq.wrapping_add(1);
            } else {
                return false;
            }
        }
    }
}

/// Read a record or the next available record.
/// # Safety
/// rb is initialized; r and its optional buffers are writable reader-owned storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_read_valid(rb: *mut Ringbuffer, mut seq: u64, r: *mut Record) -> bool {
    // SAFETY: (U1) the caller satisfies the C reader output-buffer contract.
    unsafe { read_valid(rb, &mut seq, r, null_mut()) }
}

/// Read record metadata and optionally count lines.
/// # Safety
/// rb is initialized; info and lines, when non-null, are writable private storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_read_valid_info(
    rb: *mut Ringbuffer,
    mut seq: u64,
    info: *mut PrintkInfo,
    lines: *mut c_uint,
) -> bool {
    let mut r = Record {
        info,
        text_buf: null_mut(),
        text_buf_size: 0,
    };
    // SAFETY: (U1) the local record forwards the caller's valid optional outputs.
    unsafe { read_valid(rb, &mut seq, &mut r, lines) }
}

/// Get the oldest available record's sequence number.
/// # Safety
/// rb and its arrays must remain alive and initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_first_valid_seq(rb: *mut Ringbuffer) -> u64 {
    let mut seq = 0;
    // SAFETY: (U1) no output buffers are requested.
    if unsafe { read_valid(rb, &mut seq, null_mut(), null_mut()) } {
        seq
    } else {
        0
    }
}

/// Get the sequence number immediately after the last available record.
/// # Safety
/// rb and its arrays must remain alive and initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_next_seq(rb: *mut Ringbuffer) -> u64 {
    // SAFETY: (U1) validated sequence reads follow C publication ordering.
    unsafe {
        let mut seq = last_finalized(rb);
        if seq != 0 {
            seq = seq.wrapping_add(1);
        }
        while read_valid(rb, &mut seq, null_mut(), null_mut()) {
            seq = seq.wrapping_add(1);
        }
        seq
    }
}

/// Reserve space for a new record.
/// # Safety
/// rb is initialized. e and r are private writer storage; the caller must commit
/// every successful reservation in the same context before re-enabling IRQs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_reserve(
    e: *mut ReservedEntry,
    rb: *mut Ringbuffer,
    r: *mut Record,
) -> bool {
    // SAFETY: (U1) C operations establish reservation ownership and save IRQ state.
    unsafe {
        let ring = addr_of_mut!((*rb).desc_ring);
        let data = addr_of_mut!((*rb).text_data_ring);
        if data_check_size(data, (*r).text_buf_size) {
            c_prb_irq_save(addr_of_mut!((*e).irqflags));
            let mut id = 0;
            if desc_reserve(rb, &mut id) {
                let d = to_desc(ring, id as u64);
                let info = to_info(ring, id as u64);
                let old_seq = read(addr_of!((*info).seq));
                c_prb_clear(info.cast(), 0, size_of::<PrintkInfo>());
                (*e).rb = rb;
                (*e).id = id;
                let index = id & (desc_count(ring) - 1);
                let seq = if old_seq == 0 && index != 0 {
                    index as u64
                } else {
                    old_seq.wrapping_add(desc_count(ring) as u64)
                };
                write(addr_of_mut!((*info).seq), seq);
                if seq != 0 {
                    desc_make_final(rb, id.wrapping_sub(1) & ID_MASK);
                }
                (*r).text_buf =
                    data_alloc(rb, (*r).text_buf_size, addr_of_mut!((*d).text_blk_lpos), id);
                if (*r).text_buf_size == 0 || !(*r).text_buf.is_null() {
                    (*r).info = info;
                    let pos = DataBlkLpos {
                        begin: read(addr_of!((*d).text_blk_lpos.begin)),
                        next: read(addr_of!((*d).text_blk_lpos.next)),
                    };
                    (*e).text_space = space_used(data, &pos);
                    return true;
                }
                prb_commit(e);
            } else {
                c_prb_inc(addr_of_mut!((*rb).fail));
                c_prb_irq_restore((*e).irqflags);
            }
        }
        c_prb_clear(r.cast(), 0, size_of::<Record>());
        false
    }
}

/// Extend the newest committed record owned by caller_id.
/// # Safety
/// Same reservation and IRQ obligations as prb_reserve().
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_reserve_in_last(
    e: *mut ReservedEntry,
    rb: *mut Ringbuffer,
    r: *mut Record,
    caller_id: u32,
    max: c_uint,
) -> bool {
    // SAFETY: (U1) the C compare-exchange obtains writer ownership before updates.
    unsafe {
        c_prb_irq_save(addr_of_mut!((*e).irqflags));
        let ring = addr_of_mut!((*rb).desc_ring);
        let data = addr_of_mut!((*rb).text_data_ring);
        let mut id = 0;
        let d = desc_reopen_last(ring, caller_id, &mut id);
        if d.is_null() {
            c_prb_irq_restore((*e).irqflags);
        } else {
            let info = to_info(ring, id as u64);
            (*e).rb = rb;
            (*e).id = id;
            let success = 'reserve: {
                if caller_id != read(addr_of!((*info).caller_id)) {
                    break 'reserve false;
                }
                let pos = DataBlkLpos {
                    begin: read(addr_of!((*d).text_blk_lpos.begin)),
                    next: read(addr_of!((*d).text_blk_lpos.next)),
                };
                let mut len = read(addr_of!((*info).text_len));
                if dataless(&pos) {
                    if c_prb_warn_9(len != 0) {
                        c_prb_warn_len_zero(len);
                        write(addr_of_mut!((*info).text_len), 0u16);
                    }
                    if !data_check_size(data, (*r).text_buf_size) || (*r).text_buf_size > max {
                        break 'reserve false;
                    }
                    (*r).text_buf =
                        data_alloc(rb, (*r).text_buf_size, addr_of_mut!((*d).text_blk_lpos), id);
                } else {
                    let mut size = 0;
                    if get_data(data, &pos, &mut size).is_null() {
                        break 'reserve false;
                    }
                    if c_prb_warn_10(c_uint::from(len) > size) {
                        c_prb_warn_len_max(len, size);
                        len = size as u16;
                        write(addr_of_mut!((*info).text_len), len);
                    }
                    (*r).text_buf_size = (*r).text_buf_size.wrapping_add(c_uint::from(len));
                    if !data_check_size(data, (*r).text_buf_size) || (*r).text_buf_size > max {
                        break 'reserve false;
                    }
                    (*r).text_buf =
                        data_realloc(rb, (*r).text_buf_size, addr_of_mut!((*d).text_blk_lpos), id);
                }
                if (*r).text_buf_size != 0 && (*r).text_buf.is_null() {
                    break 'reserve false;
                }
                (*r).info = info;
                let pos = DataBlkLpos {
                    begin: read(addr_of!((*d).text_blk_lpos.begin)),
                    next: read(addr_of!((*d).text_blk_lpos.next)),
                };
                (*e).text_space = space_used(data, &pos);
                true
            };
            if success {
                return true;
            }
            prb_commit(e);
        }
        c_prb_clear(r.cast(), 0, size_of::<Record>());
        false
    }
}

/// Initialize caller-provided descriptor, metadata and text arrays.
/// # Safety
/// All arrays have the power-of-two capacities supplied and valid C alignment.
/// rb and its arrays are exclusively owned until initialization returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_init(
    rb: *mut Ringbuffer,
    text: *mut c_char,
    textbits: c_uint,
    descs: *mut Desc,
    descbits: c_uint,
    infos: *mut PrintkInfo,
) {
    // SAFETY: (U7) the caller supplies exclusively owned C storage for initialization.
    unsafe {
        let count = 1usize << descbits;
        let id = (count as c_ulong).wrapping_add(1).wrapping_neg() & ID_MASK;
        c_prb_clear(descs.cast(), 0, count * size_of::<Desc>());
        c_prb_clear(infos.cast(), 0, count * size_of::<PrintkInfo>());
        (*rb).desc_ring.count_bits = descbits;
        (*rb).desc_ring.descs = descs;
        (*rb).desc_ring.infos = infos;
        c_prb_store(addr_of_mut!((*rb).desc_ring.head_id), id as c_long);
        c_prb_store(addr_of_mut!((*rb).desc_ring.tail_id), id as c_long);
        c_prb_store(addr_of_mut!((*rb).desc_ring.last_finalized_seq), 0);
        (*rb).text_data_ring.size_bits = textbits;
        (*rb).text_data_ring.data = text;
        let lpos = (1 as c_ulong).wrapping_shl(textbits).wrapping_neg();
        c_prb_store(addr_of_mut!((*rb).text_data_ring.head_lpos), lpos as c_long);
        c_prb_store(addr_of_mut!((*rb).text_data_ring.tail_lpos), lpos as c_long);
        c_prb_store(addr_of_mut!((*rb).fail), 0);
        let last = descs.add(count - 1);
        c_prb_store(addr_of_mut!((*last).state_var), sv(id, REUSABLE) as c_long);
        (*last).text_blk_lpos.begin = FAILED_LPOS;
        (*last).text_blk_lpos.next = FAILED_LPOS;
        (*infos).seq = (count as u64).wrapping_neg();
        (*infos.add(count - 1)).seq = 0;
    }
}

/// Query total text-ring space consumed by a successful reservation.
/// # Safety
/// e belongs to the caller and contains a successful reservation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_record_text_space(e: *mut ReservedEntry) -> c_uint {
    // SAFETY: (U3) the writer owns this initialized reservation handle.
    unsafe { (*e).text_space }
}
const COMMITTED: i32 = 1;
const FINALIZED: i32 = 2;
const REUSABLE: i32 = 3;
const MISS: i32 = -1;
const FLAGS_SHIFT: u32 = c_ulong::BITS - 2;
const ID_MASK: c_ulong = c_ulong::MAX >> 2;
const FAILED_LPOS: c_ulong = 1;
fn dataless(lpos: &DataBlkLpos) -> bool {
    lpos.begin & lpos.next & 1 != 0
}

/// # Safety
/// ring is an initialized data ring.
unsafe fn wrapped(ring: *const DataRing, begin: c_ulong, next: c_ulong) -> bool {
    // SAFETY: (U3) size_bits is immutable for the lifetime of the ring.
    unsafe { begin >> (*ring).size_bits != next.wrapping_sub(1) >> (*ring).size_bits }
}

/// # Safety
/// ring is an initialized data ring.
unsafe fn next_lpos(ring: *const DataRing, begin: c_ulong, size: c_uint) -> c_ulong {
    let next = begin.wrapping_add(c_ulong::from(size));
    // SAFETY: (U3) only the ring's immutable geometry is inspected.
    unsafe {
        if !wrapped(ring, begin, next) {
            next
        } else {
            (next & !(data_size(ring) - 1)).wrapping_add(c_ulong::from(size))
        }
    }
}

/// # Safety
/// rb is alive and id is held reserved; lpos is its writable block location.
unsafe fn data_alloc(
    rb: *mut Ringbuffer,
    size: c_uint,
    lpos: *mut DataBlkLpos,
    id: c_ulong,
) -> *mut c_char {
    // SAFETY: (U1) payload writes/copies and state changes use the C access layer.
    unsafe {
        if size == 0 {
            write(addr_of_mut!((*lpos).begin), EMPTY_LINE_LPOS);
            write(addr_of_mut!((*lpos).next), EMPTY_LINE_LPOS);
            return null_mut();
        }
        let ring = addr_of_mut!((*rb).text_data_ring);
        let size = to_blk_size(size);
        let mut begin = load(addr_of!((*ring).head_lpos));
        let next = loop {
            let next = next_lpos(ring, begin, size);
            if c_prb_warn_2(next.wrapping_sub(begin) > data_size(ring))
                || !data_push_tail(rb, next.wrapping_sub(data_size(ring)))
            {
                write(addr_of_mut!((*lpos).begin), FAILED_LPOS);
                write(addr_of_mut!((*lpos).next), FAILED_LPOS);
                return null_mut();
            }
            if cas(addr_of_mut!((*ring).head_lpos), &mut begin, next) {
                break next;
            }
            // 1. Guarantee any descriptor states that have transitioned
            //    to reusable are stored before modifying the newly
            //    allocated data area. A full memory barrier is needed
            //    since other CPUs may have made the descriptor states
            //    reusable. See data_push_tail:A about why the reusable
            //    states are visible. This pairs with desc_read:D.
            //
            // 2. Guarantee any updated tail lpos is stored before
            //    modifying the newly allocated data area. Another CPU may
            //    be in data_make_reusable() and is reading a block ID
            //    from this area. data_make_reusable() can handle reading
            //    a garbage block ID value, but then it must be able to
            //    load a new tail lpos. A full memory barrier is needed
            //    since other CPUs may have updated the tail lpos. This
            //    pairs with data_push_tail:B.
        }; // LMM(data_alloc:A)
        let mut block = to_block(ring, begin);
        write(block, id); // LMM(data_alloc:B)
        if wrapped(ring, begin, next) {
            block = to_block(ring, 0);
            write(block, id);
        }
        write(addr_of_mut!((*lpos).begin), begin);
        write(addr_of_mut!((*lpos).next), next);
        block.add(1).cast()
    }
}

/// # Safety
/// rb is alive; the caller has reopened id and owns the block.
unsafe fn data_realloc(
    rb: *mut Ringbuffer,
    size: c_uint,
    lpos: *mut DataBlkLpos,
    id: c_ulong,
) -> *mut c_char {
    // SAFETY: (U1) C accessors preserve the ring protocol for recycled memory.
    unsafe {
        let ring = addr_of_mut!((*rb).text_data_ring);
        let begin = read(addr_of!((*lpos).begin));
        let end = read(addr_of!((*lpos).next));
        let mut head = load(addr_of!((*ring).head_lpos));
        if head != end {
            return null_mut();
        }
        let was_wrapped = wrapped(ring, begin, end);
        let next = next_lpos(ring, begin, to_blk_size(size));
        if !need_more_space(ring, head, next) {
            return to_block(ring, if was_wrapped { 0 } else { begin })
                .add(1)
                .cast();
        }
        if c_prb_warn_3(next.wrapping_sub(begin) > data_size(ring))
            || !data_push_tail(rb, next.wrapping_sub(data_size(ring)))
        {
            return null_mut();
        }
        if !cas(addr_of_mut!((*ring).head_lpos), &mut head, next) {
            return null_mut();
        }
        let mut block = to_block(ring, begin);
        if wrapped(ring, begin, next) {
            let old = block;
            block = to_block(ring, 0);
            write(block, id);
            if !was_wrapped {
                c_prb_copy(
                    block.add(1).cast(),
                    old.add(1).cast(),
                    end.wrapping_sub(begin) as usize - size_of::<c_ulong>(),
                );
            }
        }
        write(addr_of_mut!((*lpos).next), next);
        block.add(1).cast()
    }
}

/// # Safety
/// ring is alive, lpos is a validated local snapshot or reserved block.
unsafe fn space_used(ring: *const DataRing, lpos: &DataBlkLpos) -> c_uint {
    if dataless(lpos) {
        return 0;
    }
    // SAFETY: (U3) the geometry is immutable and the snapshot is validated.
    unsafe {
        let size = data_size(ring);
        let begin = lpos.begin & (size - 1);
        let end = lpos.next & (size - 1);
        if !wrapped(ring, lpos.begin, lpos.next) {
            end.wrapping_sub(begin) as c_uint
        } else {
            end.wrapping_add(size).wrapping_sub(begin) as c_uint
        }
    }
}

/// # Safety
/// ring is initialized. lpos is a snapshot; returned memory remains speculative.
unsafe fn get_data(ring: *const DataRing, lpos: &DataBlkLpos, size: &mut c_uint) -> *const c_char {
    // SAFETY: (U1) warnings use the C boundary; block geometry is checked before use.
    unsafe {
        if dataless(lpos) {
            if lpos.begin == EMPTY_LINE_LPOS && lpos.next == EMPTY_LINE_LPOS {
                *size = 0;
                return c"".to_bytes_with_nul().as_ptr().cast();
            }
            return null_mut();
        }
        let block;
        if !wrapped(ring, lpos.begin, lpos.next) {
            block = to_block(ring, lpos.begin);
            *size = lpos.next.wrapping_sub(lpos.begin) as c_uint;
        } else if !wrapped(ring, lpos.begin.wrapping_add(data_size(ring)), lpos.next) {
            block = to_block(ring, 0);
            *size = (lpos.next & (data_size(ring) - 1)) as c_uint;
        } else {
            c_prb_warn_4(true);
            return null_mut();
        }
        let mask = size_of::<c_ulong>() as c_ulong - 1;
        if c_prb_warn_5(lpos.begin & mask != 0) || c_prb_warn_6(lpos.next & mask != 0) {
            return null_mut();
        }
        if c_prb_warn_7(*size as usize <= size_of::<c_ulong>()) {
            return null_mut();
        }
        *size -= size_of::<c_ulong>() as c_uint;
        if c_prb_warn_8(!data_check_size(ring, *size)) {
            return null_mut();
        }
        block.add(1).cast()
    }
}

/// # Safety
/// ring is alive; caller requests ownership of its last committed descriptor.
unsafe fn desc_reopen_last(ring: *mut DescRing, caller: u32, id_out: &mut c_ulong) -> *mut Desc {
    // SAFETY: (U1) the C compare-exchange acquires the reserved descriptor state.
    unsafe {
        let id = load(addr_of!((*ring).head_id));
        let mut desc = empty_desc();
        let mut cid = 0;
        if desc_read(ring, id, &mut desc, null_mut(), &mut cid) != COMMITTED || cid != caller {
            return null_mut();
        }
        let d = to_desc(ring, id as u64);
        let mut old = sv(id, COMMITTED);
        if !cas(addr_of_mut!((*d).state_var), &mut old, sv(id, RESERVED)) {
            return null_mut();
        }
        *id_out = id;
        d
    }
}

/// # Safety
/// rb is an initialized ringbuffer that remains alive.
unsafe fn last_finalized(rb: *mut Ringbuffer) -> u64 {
    // SAFETY: (U1) acquire load pairs with the C release publication protocol.
    unsafe {
        let seq = c_prb_load_acquire(addr_of!((*rb).desc_ring.last_finalized_seq)) as c_ulong;
        #[cfg(target_pointer_width = "64")]
        {
            seq as u64
        }
        #[cfg(target_pointer_width = "32")]
        {
            let first = prb_first_seq(rb);
            first.wrapping_sub((first as u32).wrapping_sub(seq as u32) as i32 as u64)
        }
    }
}

/// # Safety
/// rb remains alive and contains initialized arrays.
unsafe fn update_last_finalized(rb: *mut Ringbuffer) {
    // SAFETY: (U1) release compare-exchange publishes only validated sequence numbers.
    unsafe {
        let mut old_seq = last_finalized(rb);
        loop {
            let mut finalized = old_seq;
            let mut next = finalized.wrapping_add(1);
            while read_valid(rb, &mut next, null_mut(), null_mut()) {
                finalized = next;
                next = next.wrapping_add(1);
            }
            if finalized == old_seq {
                return;
            }
            let mut old = old_seq as c_long;
            if c_prb_cas_release(
                addr_of_mut!((*rb).desc_ring.last_finalized_seq),
                &mut old,
                finalized as c_long,
            ) {
                return;
            }
            #[cfg(target_pointer_width = "64")]
            {
                old_seq = old as u64;
            }
            #[cfg(target_pointer_width = "32")]
            {
                let first = prb_first_seq(rb);
                old_seq = first.wrapping_sub((first as u32).wrapping_sub(old as u32) as i32 as u64);
            }
        }
    }
}

/// # Safety
/// rb is alive; id refers to a descriptor that may be finalized.
unsafe fn desc_make_final(rb: *mut Ringbuffer, id: c_ulong) {
    // SAFETY: (U1) the C compare-exchange respects descriptor ID and state.
    unsafe {
        let d = to_desc(addr_of_mut!((*rb).desc_ring), id as u64);
        let mut old = sv(id, COMMITTED) as c_long;
        if c_prb_cas_relaxed(
            addr_of_mut!((*d).state_var),
            &mut old,
            sv(id, FINALIZED) as c_long,
        ) {
            update_last_finalized(rb);
        }
    }
}

/// # Safety
/// e owns a successfully reserved entry with saved IRQ flags.
unsafe fn commit(e: *mut ReservedEntry, s: i32) {
    // SAFETY: (U1) C atomic commit publishes the writer's data before restoring IRQs.
    unsafe {
        let d = to_desc(addr_of_mut!((*(*e).rb).desc_ring), (*e).id as u64);
        let mut old = sv((*e).id, RESERVED);
        if !cas(addr_of_mut!((*d).state_var), &mut old, sv((*e).id, s)) {
            c_prb_warn_11(true);
        }
        c_prb_irq_restore((*e).irqflags);
    }
}

/// Commit a reserved record, leaving it extendable while it remains newest.
/// # Safety
/// e must be a successfully reserved entry belonging to this execution context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_commit(e: *mut ReservedEntry) {
    // SAFETY: (U1) the caller holds the reservation through the C commit boundary.
    unsafe {
        let rb = (*e).rb;
        commit(e, COMMITTED);
        if load(addr_of!((*rb).desc_ring.head_id)) != (*e).id {
            desc_make_final(rb, (*e).id);
        }
    }
}

/// Commit and finalize a reserved record.
/// # Safety
/// e must be a successfully reserved entry belonging to this execution context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prb_final_commit(e: *mut ReservedEntry) {
    // SAFETY: (U1) the caller owns the reservation being finalized.
    unsafe {
        commit(e, FINALIZED);
        update_last_finalized((*e).rb);
    }
}
const EMPTY_LINE_LPOS: c_ulong = 3;

// Snapshots of shared C data must consist only of initialized integer bytes.
// This helper never creates a reference to the C allocation.
/// # Safety
/// Both ranges must be valid for T and T must admit all source bit patterns.
/// The C memcpy access must satisfy the documented LKMM snapshot contract.
unsafe fn read<T: Copy>(src: *const T) -> T {
    let mut out = MaybeUninit::<T>::uninit();
    // SAFETY: (U1) the caller provides a valid C snapshot source and local output.
    unsafe { c_prb_copy(out.as_mut_ptr().cast(), src.cast(), size_of::<T>()) };
    // SAFETY: (U3) the private snapshot is initialized by memcpy above; the
    // caller guarantees that every source bit pattern is a valid T.
    unsafe { out.as_ptr().read() }
}

/// # Safety
/// dst is writable C storage for T; the caller owns the reservation or init.
unsafe fn write<T: Copy>(dst: *mut T, value: T) {
    // SAFETY: (U1) memcpy forwards the reservation owner's scalar write.
    unsafe { c_prb_copy(dst.cast(), addr_of!(value).cast(), size_of::<T>()) };
}

fn sv(id: c_ulong, state: i32) -> c_ulong {
    id | ((state as c_ulong) << FLAGS_SHIFT)
}
fn state(id: c_ulong, value: c_ulong) -> i32 {
    if value & ID_MASK != id {
        MISS
    } else {
        (value >> FLAGS_SHIFT) as i32
    }
}

/// # Safety
/// p points to an initialized atomic_long_t governed by the C ring protocol.
unsafe fn load(p: *const c_long) -> c_ulong {
    // SAFETY: (U1) atomic_long_read preserves the kernel's LKMM access.
    unsafe { c_prb_load(p) as c_ulong }
}

/// # Safety
/// p is a C atomic_long_t; old is a private snapshot for the compare-exchange.
unsafe fn cas(p: *mut c_long, old: &mut c_ulong, new: c_ulong) -> bool {
    let mut expected = *old as c_long;
    // SAFETY: (U1) C updates only the supplied atomic and local expected value.
    let ok = unsafe { c_prb_cas(p, &mut expected, new as c_long) };
    *old = expected as c_ulong;
    ok
}

/// # Safety
/// ring is initialized and its immutable configuration remains alive.
unsafe fn data_size(ring: *const DataRing) -> c_ulong {
    // SAFETY: (U3) size_bits is fixed throughout the ring's active lifetime.
    1 << unsafe { (*ring).size_bits }
}

/// # Safety
/// ring is initialized and its immutable configuration remains alive.
unsafe fn desc_count(ring: *const DescRing) -> c_ulong {
    // SAFETY: (U3) count_bits is fixed throughout the ring's active lifetime.
    1 << unsafe { (*ring).count_bits }
}

/// # Safety
/// ring's descriptor array is alive and contains 2^count_bits entries.
unsafe fn to_desc(ring: *const DescRing, n: u64) -> *mut Desc {
    // SAFETY: (U3) masking confines the index to the initialized C array.
    unsafe {
        (*ring)
            .descs
            .add((n as c_ulong & (desc_count(ring) - 1)) as usize)
    }
}

/// # Safety
/// ring's metadata array is alive and contains 2^count_bits entries.
unsafe fn to_info(ring: *const DescRing, n: u64) -> *mut PrintkInfo {
    // SAFETY: (U3) masking confines the index to the initialized C array.
    unsafe {
        (*ring)
            .infos
            .add((n as c_ulong & (desc_count(ring) - 1)) as usize)
    }
}

/// # Safety
/// ring's text array is alive and contains 2^size_bits bytes.
unsafe fn to_block(ring: *const DataRing, lpos: c_ulong) -> *mut c_ulong {
    // SAFETY: (U3) the caller's valid lpos and the mask select the C data block.
    unsafe {
        (*ring)
            .data
            .add((lpos & (data_size(ring) - 1)) as usize)
            .cast()
    }
}

fn to_blk_size(size: c_uint) -> c_uint {
    let word = size_of::<c_ulong>() as c_uint;
    size.wrapping_add(word).wrapping_add(word - 1) & !(word - 1)
}

/// # Safety
/// ring is a valid initialized data ring.
unsafe fn data_check_size(ring: *const DataRing, size: c_uint) -> bool {
    // SAFETY: (U3) size_bits is immutable.
    size == 0 || c_ulong::from(to_blk_size(size)) <= unsafe { data_size(ring) } / 2
}

/// # Safety
/// ring is a valid initialized data ring.
unsafe fn need_more_space(ring: *const DataRing, current: c_ulong, target: c_ulong) -> bool {
    // SAFETY: (U3) size_bits is immutable.
    target.wrapping_sub(current).wrapping_sub(1) < unsafe { data_size(ring) }
}

/// # Safety
/// ring and its arrays remain alive. Output pointers, when non-null, are private.
unsafe fn desc_read(
    ring: *mut DescRing,
    id: c_ulong,
    out: *mut Desc,
    seq: *mut u64,
    caller: *mut u32,
) -> i32 {
    // SAFETY: (U1) all shared snapshots and barriers use the C LKMM boundary;
    // outputs belong to this reader. No shared payload reference is formed.
    unsafe {
        let desc = to_desc(ring, id as u64);
        let info = to_info(ring, id as u64);
        let mut value = load(addr_of!((*desc).state_var)); // LMM(desc_read:A)
        let mut s = state(id, value);
        if s != MISS && s != RESERVED {
            // Guarantee the state is loaded before copying the descriptor
            // content. This avoids copying obsolete descriptor content that might
            // not apply to the descriptor state. This pairs with _prb_commit:B.
            //
            // Memory barrier involvement:
            //
            // If desc_read:A reads from _prb_commit:B, then desc_read:C reads
            // from _prb_commit:A.
            //
            // Relies on:
            //
            // WMB from _prb_commit:A to _prb_commit:B
            //    matching
            // RMB from desc_read:A to desc_read:C
            c_prb_rmb(); // LMM(desc_read:B)
            if !out.is_null() {
                c_prb_copy(
                    addr_of_mut!((*out).text_blk_lpos).cast(),
                    addr_of!((*desc).text_blk_lpos).cast(),
                    size_of::<DataBlkLpos>(),
                );
                // Copy the descriptor data. The data is not valid until the
                // state has been re-checked. A memcpy() for all of @desc
                // cannot be used because of the atomic_t @state_var field.
            } // LMM(desc_read:C)
            if !seq.is_null() {
                *seq = read(addr_of!((*info).seq));
            }
            if !caller.is_null() {
                *caller = read(addr_of!((*info).caller_id));
            }
            // 1. Guarantee the descriptor content is loaded before re-checking
            //    the state. This avoids reading an obsolete descriptor state
            //    that may not apply to the copied content. This pairs with
            //    desc_reserve:F.
            //
            //    Memory barrier involvement:
            //
            //    If desc_read:C reads from desc_reserve:G, then desc_read:E
            //    reads from desc_reserve:F.
            //
            //    Relies on:
            //
            //    WMB from desc_reserve:F to desc_reserve:G
            //       matching
            //    RMB from desc_read:C to desc_read:E
            //
            // 2. Guarantee the record data is loaded before re-checking the
            //    state. This avoids reading an obsolete descriptor state that may
            //    not apply to the copied data. This pairs with data_alloc:A and
            //    data_realloc:A.
            //
            //    Memory barrier involvement:
            //
            //    If copy_data:A reads from data_alloc:B, then desc_read:E
            //    reads from desc_make_reusable:A.
            //
            //    Relies on:
            //
            //    MB from desc_make_reusable:A to data_alloc:B
            //       matching
            //    RMB from desc_read:C to desc_read:E
            //
            //    Note: desc_make_reusable:A and data_alloc:B can be different
            //          CPUs. However, the data_alloc:B CPU (which performs the
            //          full memory barrier) must have previously seen
            //          desc_make_reusable:A.
            c_prb_rmb(); // LMM(desc_read:D)
                         // The data has been copied. Return the current descriptor state,
                         // which may have changed since the load above.
            value = load(addr_of!((*desc).state_var)); // LMM(desc_read:E)
            s = state(id, value);
        }
        if !out.is_null() {
            (*out).state_var = value as c_long;
        }
        s
    }
}

/// # Safety
/// ring and its descriptors remain alive throughout the transition.
unsafe fn desc_make_reusable(ring: *mut DescRing, id: c_ulong) {
    // SAFETY: (U1) the C cmpxchg conditionally invalidates the requested ID.
    unsafe {
        let d = to_desc(ring, id as u64);
        let mut old = sv(id, FINALIZED) as c_long;
        c_prb_cas_relaxed(
            addr_of_mut!((*d).state_var),
            &mut old,
            sv(id, REUSABLE) as c_long,
        );
    }
}

fn empty_desc() -> Desc {
    Desc {
        state_var: 0,
        text_blk_lpos: DataBlkLpos { begin: 0, next: 0 },
    }
}

/// # Safety
/// rb and arrays are initialized; out is private writable storage.
unsafe fn data_make_reusable(
    rb: *mut Ringbuffer,
    mut begin: c_ulong,
    end: c_ulong,
    out: &mut c_ulong,
) -> bool {
    // SAFETY: (U1) C accessors read racing payload and preserve descriptor ordering.
    unsafe {
        let data = addr_of_mut!((*rb).text_data_ring);
        let ring = addr_of_mut!((*rb).desc_ring);
        while need_more_space(data, begin, end) {
            // Load the block ID from the data block. This is a data race
            // against a writer that may have newly reserved this data
            // area. If the loaded value matches a valid descriptor ID,
            // the blk_lpos of that descriptor will be checked to make
            // sure it points back to this data block. If the check fails,
            // the data area has been recycled by another writer.
            let id = read(to_block(data, begin)); // LMM(data_make_reusable:A)
            let mut desc = empty_desc();
            match desc_read(ring, id, &mut desc, null_mut(), null_mut()) {
                FINALIZED => {
                    if desc.text_blk_lpos.begin != begin {
                        return false;
                    }
                    desc_make_reusable(ring, id);
                }
                REUSABLE => {
                    if desc.text_blk_lpos.begin != begin {
                        return false;
                    }
                }
                _ => return false,
            }
            begin = desc.text_blk_lpos.next;
        }
        *out = begin;
        true
    }
}

/// # Safety
/// rb and arrays remain valid throughout concurrent recycling.
unsafe fn data_push_tail(rb: *mut Ringbuffer, lpos: c_ulong) -> bool {
    if lpos & 1 != 0 {
        return true;
    }
    // SAFETY: (U1) the C atomics and barriers preserve the original tail protocol.
    unsafe {
        let ring = addr_of_mut!((*rb).text_data_ring);
        // Any descriptor states that have transitioned to reusable due to the
        // data tail being pushed to this loaded value will be visible to this
        // CPU. This pairs with data_push_tail:D.
        //
        // Memory barrier involvement:
        //
        // If data_push_tail:A reads from data_push_tail:D, then this CPU can
        // see desc_make_reusable:A.
        //
        // Relies on:
        //
        // MB from desc_make_reusable:A to data_push_tail:D
        //    matches
        // READFROM from data_push_tail:D to data_push_tail:A
        //    thus
        // READFROM from desc_make_reusable:A to this CPU
        let mut tail = load(addr_of!((*ring).tail_lpos)); // LMM(data_push_tail:A)
        while need_more_space(ring, tail, lpos) {
            let mut next = 0;
            if !data_make_reusable(rb, tail, lpos, &mut next) {
                // 1. Guarantee the block ID loaded in
                //    data_make_reusable() is performed before
                //    reloading the tail lpos. The failed
                //    data_make_reusable() may be due to a newly
                //    recycled data area causing the tail lpos to
                //    have been previously pushed. This pairs with
                //    data_alloc:A and data_realloc:A.
                //
                //    Memory barrier involvement:
                //
                //    If data_make_reusable:A reads from data_alloc:B,
                //    then data_push_tail:C reads from
                //    data_push_tail:D.
                //
                //    Relies on:
                //
                //    MB from data_push_tail:D to data_alloc:B
                //       matching
                //    RMB from data_make_reusable:A to
                //    data_push_tail:C
                //
                //    Note: data_push_tail:D and data_alloc:B can be
                //          different CPUs. However, the data_alloc:B
                //          CPU (which performs the full memory
                //          barrier) must have previously seen
                //          data_push_tail:D.
                //
                // 2. Guarantee the descriptor state loaded in
                //    data_make_reusable() is performed before
                //    reloading the tail lpos. The failed
                //    data_make_reusable() may be due to a newly
                //    recycled descriptor causing the tail lpos to
                //    have been previously pushed. This pairs with
                //    desc_reserve:D.
                //
                //    Memory barrier involvement:
                //
                //    If data_make_reusable:B reads from
                //    desc_reserve:F, then data_push_tail:C reads
                //    from data_push_tail:D.
                //
                //    Relies on:
                //
                //    MB from data_push_tail:D to desc_reserve:F
                //       matching
                //    RMB from data_make_reusable:B to
                //    data_push_tail:C
                //
                //    Note: data_push_tail:D and desc_reserve:F can
                //          be different CPUs. However, the
                //          desc_reserve:F CPU (which performs the
                //          full memory barrier) must have previously
                //          seen data_push_tail:D.
                c_prb_rmb(); // LMM(data_push_tail:B)
                let new_tail = load(addr_of!((*ring).tail_lpos)); // LMM(data_push_tail:C)
                if new_tail == tail {
                    return false;
                }
                tail = new_tail;
                continue;
            }
            if cas(addr_of_mut!((*ring).tail_lpos), &mut tail, next) {
                break;
            }
            // Guarantee any descriptor states that have transitioned to
            // reusable are stored before pushing the tail lpos. A full
            // memory barrier is needed since other CPUs may have made
            // the descriptor states reusable. This pairs with
            // data_push_tail:A.
        } // LMM(data_push_tail:D)
        true
    }
}

/// # Safety
/// rb and arrays remain valid throughout concurrent recycling.
unsafe fn desc_push_tail(rb: *mut Ringbuffer, tail: c_ulong) -> bool {
    // SAFETY: (U1) all shared state transitions use the original C atomics.
    unsafe {
        let ring = addr_of_mut!((*rb).desc_ring);
        let mut desc = empty_desc();
        match desc_read(ring, tail, &mut desc, null_mut(), null_mut()) {
            MISS => {
                return (desc.state_var as c_ulong & ID_MASK)
                    != (tail.wrapping_sub(desc_count(ring)) & ID_MASK)
            }
            RESERVED | COMMITTED => return false,
            FINALIZED => desc_make_reusable(ring, tail),
            _ => {}
        }
        if !data_push_tail(rb, desc.text_blk_lpos.next) {
            return false;
        }
        let next = tail.wrapping_add(1) & ID_MASK;
        // Check the next descriptor after @tail_id before pushing the tail
        // to it because the tail must always be in a finalized or reusable
        // state. The implementation of prb_first_seq() relies on this.
        //
        // A successful read implies that the next descriptor is less than or
        // equal to @head_id so there is no risk of pushing the tail past the
        // head.
        let s = desc_read(ring, next, &mut desc, null_mut(), null_mut()); // LMM(desc_push_tail:A)
        if s == FINALIZED || s == REUSABLE {
            let mut expected = tail;
            // Guarantee any descriptor states that have transitioned to
            // reusable are stored before pushing the tail ID. This allows
            // verifying the recycled descriptor state. A full memory
            // barrier is needed since other CPUs may have made the
            // descriptor states reusable. This pairs with desc_reserve:D.
            cas(addr_of_mut!((*ring).tail_id), &mut expected, next); // LMM(desc_push_tail:B)
        } else {
            // Guarantee the last state load from desc_read() is before
            // reloading @tail_id in order to see a new tail ID in the
            // case that the descriptor has been recycled. This pairs
            // with desc_reserve:D.
            //
            // Memory barrier involvement:
            //
            // If desc_push_tail:A reads from desc_reserve:F, then
            // desc_push_tail:D reads from desc_push_tail:B.
            //
            // Relies on:
            //
            // MB from desc_push_tail:B to desc_reserve:F
            //    matching
            // RMB from desc_push_tail:A to desc_push_tail:D
            //
            // Note: desc_push_tail:B and desc_reserve:F can be different
            //       CPUs. However, the desc_reserve:F CPU (which performs
            //       the full memory barrier) must have previously seen
            //       desc_push_tail:B.
            c_prb_rmb(); // LMM(desc_push_tail:C)
            if load(addr_of!((*ring).tail_id)) == tail {
                return false;
            }
        }
        true
    }
}

/// # Safety
/// rb and arrays remain alive; id_out is private storage for the reservation.
unsafe fn desc_reserve(rb: *mut Ringbuffer, id_out: &mut c_ulong) -> bool {
    // SAFETY: (U1) C atomics/barriers implement the reservation ownership protocol.
    unsafe {
        let ring = addr_of_mut!((*rb).desc_ring);
        let mut head = load(addr_of!((*ring).head_id));
        let (id, previous) = loop {
            let id = head.wrapping_add(1) & ID_MASK;
            let previous = id.wrapping_sub(desc_count(ring)) & ID_MASK;
            // Guarantee the head ID is read before reading the tail ID.
            // Since the tail ID is updated before the head ID, this
            // guarantees that @id_prev_wrap is never ahead of the tail
            // ID. This pairs with desc_reserve:D.
            //
            // Memory barrier involvement:
            //
            // If desc_reserve:A reads from desc_reserve:D, then
            // desc_reserve:C reads from desc_push_tail:B.
            //
            // Relies on:
            //
            // MB from desc_push_tail:B to desc_reserve:D
            //    matching
            // RMB from desc_reserve:A to desc_reserve:C
            //
            // Note: desc_push_tail:B and desc_reserve:D can be different
            //       CPUs. However, the desc_reserve:D CPU (which performs
            //       the full memory barrier) must have previously seen
            //       desc_push_tail:B.
            c_prb_rmb(); // LMM(desc_reserve:B)
            if previous == load(addr_of!((*ring).tail_id)) && !desc_push_tail(rb, previous) {
                return false;
            }
            if cas(addr_of_mut!((*ring).head_id), &mut head, id) {
                break (id, previous);
            }
            // 1. Guarantee the tail ID is read before validating the
            //    recycled descriptor state. A read memory barrier is
            //    sufficient for this. This pairs with desc_push_tail:B.
            //
            //    Memory barrier involvement:
            //
            //    If desc_reserve:C reads from desc_push_tail:B, then
            //    desc_reserve:E reads from desc_make_reusable:A.
            //
            //    Relies on:
            //
            //    MB from desc_make_reusable:A to desc_push_tail:B
            //       matching
            //    RMB from desc_reserve:C to desc_reserve:E
            //
            //    Note: desc_make_reusable:A and desc_push_tail:B can be
            //          different CPUs. However, the desc_push_tail:B CPU
            //          (which performs the full memory barrier) must have
            //          previously seen desc_make_reusable:A.
            //
            // 2. Guarantee the tail ID is stored before storing the head
            //    ID. This pairs with desc_reserve:B.
            //
            // 3. Guarantee any data ring tail changes are stored before
            //    recycling the descriptor. Data ring tail changes can
            //    happen via desc_push_tail()->data_push_tail(). A full
            //    memory barrier is needed since another CPU may have
            //    pushed the data ring tails. This pairs with
            //    data_push_tail:B.
            //
            // 4. Guarantee a new tail ID is stored before recycling the
            //    descriptor. A full memory barrier is needed since
            //    another CPU may have pushed the tail ID. This pairs
            //    with desc_push_tail:C and this also pairs with
            //    prb_first_seq:C.
            //
            // 5. Guarantee the head ID is stored before trying to
            //    finalize the previous descriptor. This pairs with
            //    _prb_commit:B.
        }; // LMM(desc_reserve:D)
        let desc = to_desc(ring, id as u64);
        // If the descriptor has been recycled, verify the old state val.
        // See "ABA Issues" about why this verification is performed.
        let mut old = load(addr_of!((*desc).state_var)); // LMM(desc_reserve:E)
        if old != 0 && state(previous, old) != REUSABLE {
            c_prb_warn_0(true);
            return false;
        }
        if !cas(addr_of_mut!((*desc).state_var), &mut old, sv(id, RESERVED)) {
            c_prb_warn_1(true);
            return false;
            // Assign the descriptor a new ID and set its state to reserved.
            // See "ABA Issues" about why cmpxchg() instead of set() is used.
            //
            // Guarantee the new descriptor ID and state is stored before making
            // any other changes. A write memory barrier is sufficient for this.
            // This pairs with desc_read:D.
        } // LMM(desc_reserve:F)
        *id_out = id;
        true
    }
}
use crate::port_layout::*;

/// Logical position and extent of a C ring data block.
#[repr(C)]
pub struct DataBlkLpos {
    begin: c_ulong,
    next: c_ulong,
}

kr::static_assert_layout!(DataBlkLpos, size = PORT_PRB_DATA_BLK_LPOS_SIZE, align = PORT_PRB_DATA_BLK_LPOS_ALIGN,
    begin @ PORT_PRB_DATA_BLK_LPOS_BEGIN,
    next @ PORT_PRB_DATA_BLK_LPOS_NEXT,
);

/// C descriptor containing publication state and text positions.
#[repr(C)]
pub struct Desc {
    state_var: c_long,
    text_blk_lpos: DataBlkLpos,
}

kr::static_assert_layout!(Desc, size = PORT_PRB_DESC_SIZE, align = PORT_PRB_DESC_ALIGN,
    state_var @ PORT_PRB_DESC_STATE_VAR,
    text_blk_lpos @ PORT_PRB_DESC_TEXT_BLK_LPOS,
);

/// C text ring geometry and atomic positions.
#[repr(C)]
pub struct DataRing {
    size_bits: c_uint,
    data: *mut c_char,
    head_lpos: c_long,
    tail_lpos: c_long,
}

kr::static_assert_layout!(DataRing, size = PORT_PRB_DATA_RING_SIZE, align = PORT_PRB_DATA_RING_ALIGN,
    size_bits @ PORT_PRB_DATA_RING_SIZE_BITS,
    data @ PORT_PRB_DATA_RING_DATA,
    head_lpos @ PORT_PRB_DATA_RING_HEAD_LPOS,
    tail_lpos @ PORT_PRB_DATA_RING_TAIL_LPOS,
);

/// C descriptor ring geometry and atomic sequence state.
#[repr(C)]
pub struct DescRing {
    count_bits: c_uint,
    descs: *mut Desc,
    infos: *mut PrintkInfo,
    head_id: c_long,
    tail_id: c_long,
    last_finalized_seq: c_long,
}

kr::static_assert_layout!(DescRing, size = PORT_PRB_DESC_RING_SIZE, align = PORT_PRB_DESC_RING_ALIGN,
    count_bits @ PORT_PRB_DESC_RING_COUNT_BITS,
    descs @ PORT_PRB_DESC_RING_DESCS,
    infos @ PORT_PRB_DESC_RING_INFOS,
    head_id @ PORT_PRB_DESC_RING_HEAD_ID,
    tail_id @ PORT_PRB_DESC_RING_TAIL_ID,
    last_finalized_seq @ PORT_PRB_DESC_RING_LAST_FINALIZED_SEQ,
);

/// C printk ring buffer shared by readers and writers.
#[repr(C)]
pub struct Ringbuffer {
    desc_ring: DescRing,
    text_data_ring: DataRing,
    fail: c_long,
}

kr::static_assert_layout!(Ringbuffer, size = PORT_PRINTK_RINGBUFFER_SIZE, align = PORT_PRINTK_RINGBUFFER_ALIGN,
    desc_ring @ PORT_PRINTK_RINGBUFFER_DESC_RING,
    text_data_ring @ PORT_PRINTK_RINGBUFFER_TEXT_DATA_RING,
    fail @ PORT_PRINTK_RINGBUFFER_FAIL,
);

/// Writer-owned reservation returned by the C ABI.
#[repr(C)]
pub struct ReservedEntry {
    rb: *mut Ringbuffer,
    irqflags: c_ulong,
    id: c_ulong,
    text_space: c_uint,
}

kr::static_assert_layout!(ReservedEntry, size = PORT_PRB_RESERVED_ENTRY_SIZE, align = PORT_PRB_RESERVED_ENTRY_ALIGN,
    rb @ PORT_PRB_RESERVED_ENTRY_RB,
    irqflags @ PORT_PRB_RESERVED_ENTRY_IRQFLAGS,
    id @ PORT_PRB_RESERVED_ENTRY_ID,
    text_space @ PORT_PRB_RESERVED_ENTRY_TEXT_SPACE,
);

/// Caller-provided metadata and text buffers.
#[repr(C)]
pub struct Record {
    info: *mut PrintkInfo,
    text_buf: *mut c_char,
    text_buf_size: c_uint,
}

kr::static_assert_layout!(Record, size = PORT_PRINTK_RECORD_SIZE, align = PORT_PRINTK_RECORD_ALIGN,
    info @ PORT_PRINTK_RECORD_INFO,
    text_buf @ PORT_PRINTK_RECORD_TEXT_BUF,
    text_buf_size @ PORT_PRINTK_RECORD_TEXT_BUF_SIZE,
);

// The record metadata is owned by printk's C writers. Its bitfields and
// configuration-dependent execution context stay opaque to the ring buffer.
// Scalar accessors below use the C-generated offsets.
/// C record metadata, including opaque execution-context fields.
#[repr(C)]
pub struct PrintkInfo {
    seq: u64,
    ts_nsec: u64,
    text_len: u16,
    facility: u8,
    flags_level: u8,
    caller_id: u32,
    execution_context: [u8; PORT_PRINTK_INFO_DEV_INFO - PORT_PRINTK_INFO_CALLER_ID - 4],
    dev_info: [u8; PORT_PRINTK_INFO_DEV_INFO_SIZE],
}

kr::static_assert_layout!(PrintkInfo,
    size = PORT_PRINTK_INFO_SIZE, align = PORT_PRINTK_INFO_ALIGN,
    seq @ PORT_PRINTK_INFO_SEQ, ts_nsec @ PORT_PRINTK_INFO_TS_NSEC,
    text_len @ PORT_PRINTK_INFO_TEXT_LEN, facility @ PORT_PRINTK_INFO_FACILITY,
    flags_level @ (PORT_PRINTK_INFO_FACILITY + 1),
    caller_id @ PORT_PRINTK_INFO_CALLER_ID,
    execution_context @ (PORT_PRINTK_INFO_CALLER_ID + 4),
    dev_info @ PORT_PRINTK_INFO_DEV_INFO,
);
