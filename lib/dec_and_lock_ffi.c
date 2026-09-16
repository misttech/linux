// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/dec_and_lock.rs: the exports of lib/dec_and_lock.c with their
 * license, and single-call wrappers for the locking macros that the Rust code
 * calls.
 */

#include <linux/export.h>
#include <linux/spinlock.h>

EXPORT_SYMBOL(atomic_dec_and_lock);
EXPORT_SYMBOL(_atomic_dec_and_lock_irqsave);
EXPORT_SYMBOL(atomic_dec_and_raw_lock);
EXPORT_SYMBOL(_atomic_dec_and_raw_lock_irqsave);

void c_dec_and_lock_spin_lock(spinlock_t *lock);
void c_dec_and_lock_spin_lock(spinlock_t *lock)
{
	spin_lock(lock);
}

void c_dec_and_lock_spin_unlock(spinlock_t *lock);
void c_dec_and_lock_spin_unlock(spinlock_t *lock)
{
	spin_unlock(lock);
}

void c_dec_and_lock_spin_lock_irqsave(spinlock_t *lock, unsigned long *flags);
void c_dec_and_lock_spin_lock_irqsave(spinlock_t *lock, unsigned long *flags)
{
	spin_lock_irqsave(lock, *flags);
}

void c_dec_and_lock_spin_unlock_irqrestore(spinlock_t *lock, unsigned long *flags);
void c_dec_and_lock_spin_unlock_irqrestore(spinlock_t *lock, unsigned long *flags)
{
	spin_unlock_irqrestore(lock, *flags);
}

void c_dec_and_lock_raw_spin_lock(raw_spinlock_t *lock);
void c_dec_and_lock_raw_spin_lock(raw_spinlock_t *lock)
{
	raw_spin_lock(lock);
}

void c_dec_and_lock_raw_spin_unlock(raw_spinlock_t *lock);
void c_dec_and_lock_raw_spin_unlock(raw_spinlock_t *lock)
{
	raw_spin_unlock(lock);
}

void c_dec_and_lock_raw_spin_lock_irqsave(raw_spinlock_t *lock, unsigned long *flags);
void c_dec_and_lock_raw_spin_lock_irqsave(raw_spinlock_t *lock, unsigned long *flags)
{
	raw_spin_lock_irqsave(lock, *flags);
}

void c_dec_and_lock_raw_spin_unlock_irqrestore(raw_spinlock_t *lock, unsigned long *flags);
void c_dec_and_lock_raw_spin_unlock_irqrestore(raw_spinlock_t *lock, unsigned long *flags)
{
	raw_spin_unlock_irqrestore(lock, *flags);
}
