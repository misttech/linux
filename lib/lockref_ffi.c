// SPDX-License-Identifier: GPL-2.0
/* Exports and C inline/macro calls used by lib/lockref.rs. */
#include <linux/export.h>
#include <linux/lockref.h>

EXPORT_SYMBOL(lockref_get);
EXPORT_SYMBOL(lockref_get_not_zero);
EXPORT_SYMBOL(lockref_put_return);
EXPORT_SYMBOL(lockref_put_or_lock);
EXPORT_SYMBOL(lockref_mark_dead);
EXPORT_SYMBOL(lockref_get_not_dead);

void c_lockref_spin_lock(struct lockref *lockref);
void c_lockref_spin_lock(struct lockref *lockref)
{
	spin_lock(&lockref->lock);
}

void c_lockref_spin_unlock(struct lockref *lockref);
void c_lockref_spin_unlock(struct lockref *lockref)
{
	spin_unlock(&lockref->lock);
}

void c_lockref_assert_spin_locked(struct lockref *lockref);
void c_lockref_assert_spin_locked(struct lockref *lockref)
{
	assert_spin_locked(&lockref->lock);
}

#if USE_CMPXCHG_LOCKREF
u64 c_lockref_read_lock_count(struct lockref *lockref);
u64 c_lockref_read_lock_count(struct lockref *lockref)
{
	return READ_ONCE(lockref->lock_count);
}

bool c_lockref_arch_unlocked(const struct lockref *snapshot);
bool c_lockref_arch_unlocked(const struct lockref *snapshot)
{
	return arch_spin_value_unlocked(snapshot->lock.rlock.raw_lock);
}

bool c_lockref_try_cmpxchg(struct lockref *lockref, u64 *old, u64 new);
bool c_lockref_try_cmpxchg(struct lockref *lockref, u64 *old, u64 new)
{
	return try_cmpxchg64_relaxed(&lockref->lock_count, old, new);
}
#endif
