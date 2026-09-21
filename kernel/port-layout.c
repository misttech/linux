// SPDX-License-Identifier: GPL-2.0
/*
 * Emit C layout facts for Rust replacements whose layout depends on config.
 * This runs after bounds.h: lockref.h needs SPINLOCK_SIZE from that header.
 */
#define COMPILE_OFFSETS
#include <linux/kbuild.h>
#include <linux/lockref.h>
#include <linux/lwq.h>
#include <linux/poison.h>
#include <linux/ratelimit_types.h>

int main(void)
{
	DEFINE(PORT_LIST_POISON1, (unsigned long)LIST_POISON1);
	DEFINE(PORT_LIST_POISON2, (unsigned long)LIST_POISON2);
	DEFINE(PORT_LWQ_SIZE, sizeof(struct lwq));
	DEFINE(PORT_LWQ_ALIGN, __alignof__(struct lwq));
	DEFINE(PORT_LWQ_LOCK_SIZE, sizeof(spinlock_t));
	OFFSET(PORT_LWQ_LOCK, lwq, lock);
	OFFSET(PORT_LWQ_READY, lwq, ready);
	OFFSET(PORT_LWQ_NEW, lwq, new);
	DEFINE(PORT_RATELIMIT_SIZE, sizeof(struct ratelimit_state));
	DEFINE(PORT_RATELIMIT_ALIGN, __alignof__(struct ratelimit_state));
	DEFINE(PORT_RATELIMIT_LOCK_SIZE, sizeof(raw_spinlock_t));
	OFFSET(PORT_RATELIMIT_LOCK, ratelimit_state, lock);
	OFFSET(PORT_RATELIMIT_INTERVAL, ratelimit_state, interval);
	OFFSET(PORT_RATELIMIT_BURST, ratelimit_state, burst);
	OFFSET(PORT_RATELIMIT_RS_N_LEFT, ratelimit_state, rs_n_left);
	OFFSET(PORT_RATELIMIT_MISSED, ratelimit_state, missed);
	OFFSET(PORT_RATELIMIT_FLAGS, ratelimit_state, flags);
	OFFSET(PORT_RATELIMIT_BEGIN, ratelimit_state, begin);
	DEFINE(PORT_RATELIMIT_MSG_ON_RELEASE, RATELIMIT_MSG_ON_RELEASE);
	DEFINE(PORT_RATELIMIT_INITIALIZED, RATELIMIT_INITIALIZED);

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
	return 0;
}
