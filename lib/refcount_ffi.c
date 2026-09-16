// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/refcount.rs: the exports of lib/refcount.c with their
 * license, and single-call wrappers for the C macros and inlines that the
 * Rust code calls.
 */

#include <linux/atomic.h>
#include <linux/bug.h>
#include <linux/export.h>
#include <linux/mutex.h>
#include <linux/refcount.h>
#include <linux/spinlock.h>

EXPORT_SYMBOL(refcount_warn_saturate);
EXPORT_SYMBOL(refcount_dec_if_one);
EXPORT_SYMBOL(refcount_dec_not_one);
EXPORT_SYMBOL(refcount_dec_and_mutex_lock);
EXPORT_SYMBOL(refcount_dec_and_lock);
EXPORT_SYMBOL(refcount_dec_and_lock_irqsave);

/*
 * One wrapper per WARN_ONCE() site in lib/refcount.c, so that each keeps its
 * own once flag.
 */
void c_refcount_warn_add_not_zero_ovf(void);
void c_refcount_warn_add_not_zero_ovf(void)
{
	WARN_ONCE(1, "refcount_t: saturated; leaking memory.\n");
}

void c_refcount_warn_add_ovf(void);
void c_refcount_warn_add_ovf(void)
{
	WARN_ONCE(1, "refcount_t: saturated; leaking memory.\n");
}

void c_refcount_warn_add_uaf(void);
void c_refcount_warn_add_uaf(void)
{
	WARN_ONCE(1, "refcount_t: addition on 0; use-after-free.\n");
}

void c_refcount_warn_sub_uaf(void);
void c_refcount_warn_sub_uaf(void)
{
	WARN_ONCE(1, "refcount_t: underflow; use-after-free.\n");
}

void c_refcount_warn_dec_leak(void);
void c_refcount_warn_dec_leak(void)
{
	WARN_ONCE(1, "refcount_t: decrement hit 0; leaking memory.\n");
}

void c_refcount_warn_unknown(void);
void c_refcount_warn_unknown(void)
{
	WARN_ONCE(1, "refcount_t: unknown saturation event!?.\n");
}

void c_refcount_warn_dec_not_one_underflow(void);
void c_refcount_warn_dec_not_one_underflow(void)
{
	WARN_ONCE(1, "refcount_t: underflow; use-after-free.\n");
}

bool c_refcount_dec_and_test(refcount_t *r);
bool c_refcount_dec_and_test(refcount_t *r)
{
	return refcount_dec_and_test(r);
}

void c_refcount_mutex_lock(struct mutex *lock);
void c_refcount_mutex_lock(struct mutex *lock)
{
	mutex_lock(lock);
}

void c_refcount_mutex_unlock(struct mutex *lock);
void c_refcount_mutex_unlock(struct mutex *lock)
{
	mutex_unlock(lock);
}

void c_refcount_spin_lock(spinlock_t *lock);
void c_refcount_spin_lock(spinlock_t *lock)
{
	spin_lock(lock);
}

void c_refcount_spin_unlock(spinlock_t *lock);
void c_refcount_spin_unlock(spinlock_t *lock)
{
	spin_unlock(lock);
}

void c_refcount_spin_lock_irqsave(spinlock_t *lock, unsigned long *flags);
void c_refcount_spin_lock_irqsave(spinlock_t *lock, unsigned long *flags)
{
	spin_lock_irqsave(lock, *flags);
}

void c_refcount_spin_unlock_irqrestore(spinlock_t *lock, unsigned long *flags);
void c_refcount_spin_unlock_irqrestore(spinlock_t *lock, unsigned long *flags)
{
	spin_unlock_irqrestore(lock, *flags);
}

/*
 * smp_acquire__after_ctrl_dep() is a barrier, not an LKMM annotation: an
 * instruction on arm64 (dmb ishld) and a compiler barrier on x86. Rust
 * orderings cannot express it, so the port calls it here.
 */
void c_refcount_acquire_after_ctrl_dep(void);
void c_refcount_acquire_after_ctrl_dep(void)
{
	smp_acquire__after_ctrl_dep();
}
