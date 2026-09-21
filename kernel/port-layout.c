// SPDX-License-Identifier: GPL-2.0
/*
 * Emit C layout facts for Rust replacements whose layout depends on config.
 * This runs after bounds.h: lockref.h needs SPINLOCK_SIZE from that header.
 */
#define COMPILE_OFFSETS
#include <linux/kbuild.h>
#include <linux/lockref.h>
#include "printk/printk_ringbuffer.h"

int main(void)
{
	DEFINE(PORT_LOCKREF_SIZE, sizeof(struct lockref));
	DEFINE(PORT_LOCKREF_ALIGN, __alignof__(struct lockref));
	OFFSET(PORT_LOCKREF_LOCK, lockref, lock);
	OFFSET(PORT_LOCKREF_COUNT, lockref, count);
	DEFINE(PORT_LOCKREF_FAST, USE_CMPXCHG_LOCKREF);
	DEFINE(PORT_LOCKREF_DEAD_VAL, __LOCKREF_DEAD_VAL);
#if USE_CMPXCHG_LOCKREF
	OFFSET(PORT_LOCKREF_LOCK_COUNT, lockref, lock_count);
#else
	DEFINE(PORT_LOCKREF_LOCK_COUNT, 0);
#endif
#ifdef CONFIG_PRINTK
	DEFINE(PORT_PRB_DATA_BLK_LPOS_SIZE, sizeof(struct prb_data_blk_lpos));
	DEFINE(PORT_PRB_DATA_BLK_LPOS_ALIGN, __alignof__(struct prb_data_blk_lpos));
	OFFSET(PORT_PRB_DATA_BLK_LPOS_BEGIN, prb_data_blk_lpos, begin);
	OFFSET(PORT_PRB_DATA_BLK_LPOS_NEXT, prb_data_blk_lpos, next);
	DEFINE(PORT_PRB_DESC_SIZE, sizeof(struct prb_desc));
	DEFINE(PORT_PRB_DESC_ALIGN, __alignof__(struct prb_desc));
	OFFSET(PORT_PRB_DESC_STATE_VAR, prb_desc, state_var);
	OFFSET(PORT_PRB_DESC_TEXT_BLK_LPOS, prb_desc, text_blk_lpos);
	DEFINE(PORT_PRB_DATA_RING_SIZE, sizeof(struct prb_data_ring));
	DEFINE(PORT_PRB_DATA_RING_ALIGN, __alignof__(struct prb_data_ring));
	OFFSET(PORT_PRB_DATA_RING_SIZE_BITS, prb_data_ring, size_bits);
	OFFSET(PORT_PRB_DATA_RING_DATA, prb_data_ring, data);
	OFFSET(PORT_PRB_DATA_RING_HEAD_LPOS, prb_data_ring, head_lpos);
	OFFSET(PORT_PRB_DATA_RING_TAIL_LPOS, prb_data_ring, tail_lpos);
	DEFINE(PORT_PRB_DESC_RING_SIZE, sizeof(struct prb_desc_ring));
	DEFINE(PORT_PRB_DESC_RING_ALIGN, __alignof__(struct prb_desc_ring));
	OFFSET(PORT_PRB_DESC_RING_COUNT_BITS, prb_desc_ring, count_bits);
	OFFSET(PORT_PRB_DESC_RING_DESCS, prb_desc_ring, descs);
	OFFSET(PORT_PRB_DESC_RING_INFOS, prb_desc_ring, infos);
	OFFSET(PORT_PRB_DESC_RING_HEAD_ID, prb_desc_ring, head_id);
	OFFSET(PORT_PRB_DESC_RING_TAIL_ID, prb_desc_ring, tail_id);
	OFFSET(PORT_PRB_DESC_RING_LAST_FINALIZED_SEQ, prb_desc_ring, last_finalized_seq);
	DEFINE(PORT_PRINTK_RINGBUFFER_SIZE, sizeof(struct printk_ringbuffer));
	DEFINE(PORT_PRINTK_RINGBUFFER_ALIGN, __alignof__(struct printk_ringbuffer));
	OFFSET(PORT_PRINTK_RINGBUFFER_DESC_RING, printk_ringbuffer, desc_ring);
	OFFSET(PORT_PRINTK_RINGBUFFER_TEXT_DATA_RING, printk_ringbuffer, text_data_ring);
	OFFSET(PORT_PRINTK_RINGBUFFER_FAIL, printk_ringbuffer, fail);
	DEFINE(PORT_PRB_RESERVED_ENTRY_SIZE, sizeof(struct prb_reserved_entry));
	DEFINE(PORT_PRB_RESERVED_ENTRY_ALIGN, __alignof__(struct prb_reserved_entry));
	OFFSET(PORT_PRB_RESERVED_ENTRY_RB, prb_reserved_entry, rb);
	OFFSET(PORT_PRB_RESERVED_ENTRY_IRQFLAGS, prb_reserved_entry, irqflags);
	OFFSET(PORT_PRB_RESERVED_ENTRY_ID, prb_reserved_entry, id);
	OFFSET(PORT_PRB_RESERVED_ENTRY_TEXT_SPACE, prb_reserved_entry, text_space);
	DEFINE(PORT_PRINTK_RECORD_SIZE, sizeof(struct printk_record));
	DEFINE(PORT_PRINTK_RECORD_ALIGN, __alignof__(struct printk_record));
	OFFSET(PORT_PRINTK_RECORD_INFO, printk_record, info);
	OFFSET(PORT_PRINTK_RECORD_TEXT_BUF, printk_record, text_buf);
	OFFSET(PORT_PRINTK_RECORD_TEXT_BUF_SIZE, printk_record, text_buf_size);
	DEFINE(PORT_PRINTK_INFO_SIZE, sizeof(struct printk_info));
	DEFINE(PORT_PRINTK_INFO_ALIGN, __alignof__(struct printk_info));
	OFFSET(PORT_PRINTK_INFO_SEQ, printk_info, seq);
	OFFSET(PORT_PRINTK_INFO_TS_NSEC, printk_info, ts_nsec);
	OFFSET(PORT_PRINTK_INFO_TEXT_LEN, printk_info, text_len);
	OFFSET(PORT_PRINTK_INFO_FACILITY, printk_info, facility);
	DEFINE(PORT_PRINTK_INFO_FLAGS_LEVEL, offsetof(struct printk_info, facility) + 1);
	OFFSET(PORT_PRINTK_INFO_CALLER_ID, printk_info, caller_id);
#ifdef CONFIG_PRINTK_EXECUTION_CTX
	OFFSET(PORT_PRINTK_INFO_CALLER_ID2, printk_info, caller_id2);
	OFFSET(PORT_PRINTK_INFO_COMM, printk_info, comm);
	DEFINE(PORT_PRINTK_INFO_COMM_SIZE, sizeof(((struct printk_info *)0)->comm));
#else
	DEFINE(PORT_PRINTK_INFO_CALLER_ID2, 0);
	DEFINE(PORT_PRINTK_INFO_COMM, 0);
	DEFINE(PORT_PRINTK_INFO_COMM_SIZE, 0);
#endif
	OFFSET(PORT_PRINTK_INFO_DEV_INFO, printk_info, dev_info);
	DEFINE(PORT_PRINTK_INFO_DEV_INFO_SIZE, sizeof(struct dev_printk_info));
#endif
	return 0;
}
