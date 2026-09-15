// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/kref.rs: single-call wrappers for the C lock functions that
 * the Rust code calls. kref exports no symbols.
 */

#include <linux/mutex.h>
#include <linux/spinlock.h>

void c_kref_mutex_unlock(struct mutex *lock);
void c_kref_mutex_unlock(struct mutex *lock)
{
	mutex_unlock(lock);
}

void c_kref_spin_unlock(spinlock_t *lock);
void c_kref_spin_unlock(spinlock_t *lock)
{
	spin_unlock(lock);
}
