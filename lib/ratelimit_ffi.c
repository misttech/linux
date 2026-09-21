// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/ratelimit.rs: the export of lib/ratelimit.c with its license,
 * and single-call wrappers for the C macros and inlines that the Rust code
 * calls.
 */

#include <linux/bug.h>
#include <linux/export.h>
#include <linux/printk.h>
#include <linux/ratelimit.h>
#include <linux/spinlock.h>

EXPORT_SYMBOL(___ratelimit);

/*
 * raw_spin_trylock_irqsave() is a macro that assigns to its flags argument,
 * so the caller passes the address of the flags it later restores.
 */
int c_ratelimit_trylock_irqsave(struct ratelimit_state *rs, unsigned long *flags);
int c_ratelimit_trylock_irqsave(struct ratelimit_state *rs, unsigned long *flags)
{
	return raw_spin_trylock_irqsave(&rs->lock, *flags);
}

void c_ratelimit_unlock_irqrestore(struct ratelimit_state *rs, unsigned long flags);
void c_ratelimit_unlock_irqrestore(struct ratelimit_state *rs, unsigned long flags)
{
	raw_spin_unlock_irqrestore(&rs->lock, flags);
}

/* The two static inlines of <linux/ratelimit.h> that lib/ratelimit.c uses. */
void c_ratelimit_inc_miss(struct ratelimit_state *rs);
void c_ratelimit_inc_miss(struct ratelimit_state *rs)
{
	ratelimit_state_inc_miss(rs);
}

int c_ratelimit_reset_miss(struct ratelimit_state *rs);
int c_ratelimit_reset_miss(struct ratelimit_state *rs)
{
	return ratelimit_state_reset_miss(rs);
}

/* The one WARN_ONCE() site of lib/ratelimit.c, so it keeps its once flag. */
void c_ratelimit_warn_negative(int interval, int burst);
void c_ratelimit_warn_negative(int interval, int burst)
{
	WARN_ONCE(1, "Negative interval (%d) or burst (%d): Uninitialized ratelimit_state structure?\n",
		  interval, burst);
}

/* printk_deferred() is a variadic macro, so its format is fixed here. */
void c_ratelimit_printk_suppressed(const char *func, int missed);
void c_ratelimit_printk_suppressed(const char *func, int missed)
{
	printk_deferred(KERN_WARNING
			"%s: %d callbacks suppressed\n", func, missed);
}
