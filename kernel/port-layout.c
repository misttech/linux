// SPDX-License-Identifier: GPL-2.0
/*
 * Emit C layout facts for Rust replacements whose layout depends on config.
 * This runs after bounds.h: lockref.h needs SPINLOCK_SIZE from that header.
 */
#define COMPILE_OFFSETS
#include <linux/kbuild.h>
#include <linux/lockref.h>

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
	return 0;
}
