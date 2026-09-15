// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/rcuref.rs: the exports of lib/rcuref.c with their license,
 * and single-call wrappers for the C macros and inlines that the Rust code
 * calls.
 */

#include <linux/atomic.h>
#include <linux/bug.h>
#include <linux/export.h>
#include <linux/preempt.h>
#include <linux/rcupdate.h>
#include <linux/rcuref.h>

EXPORT_SYMBOL_GPL(rcuref_get_slowpath);
EXPORT_SYMBOL_GPL(rcuref_put_slowpath);

/*
 * One wrapper per WARN_ONCE() site in lib/rcuref.c, so that each keeps its
 * own once flag.
 */
void c_rcuref_warn_saturated(void);
void c_rcuref_warn_saturated(void)
{
	WARN_ONCE(1, "rcuref saturated - leaking memory");
}

void c_rcuref_warn_imbalanced_put(void);
void c_rcuref_warn_imbalanced_put(void)
{
	WARN_ONCE(1, "rcuref - imbalanced put()");
}

/*
 * smp_acquire__after_ctrl_dep() is a barrier, not an LKMM annotation: an
 * instruction on arm64 (dmb ishld) and a compiler barrier on x86. Rust
 * orderings cannot express it, so the port calls it here.
 */
void c_rcuref_acquire_after_ctrl_dep(void);
void c_rcuref_acquire_after_ctrl_dep(void)
{
	smp_acquire__after_ctrl_dep();
}

/*
 * The RCU_LOCKDEP_WARN() and the preemption disable of the rcuref_put()
 * inlines, which the typed Rust API implements in Rust.
 */
void c_rcuref_lockdep_warn_put(void);
void c_rcuref_lockdep_warn_put(void)
{
	RCU_LOCKDEP_WARN(!rcu_read_lock_held() && preemptible(),
			 "suspicious rcuref_put_rcusafe() usage");
}

void c_rcuref_preempt_disable(void);
void c_rcuref_preempt_disable(void)
{
	preempt_disable();
}

void c_rcuref_preempt_enable(void);
void c_rcuref_preempt_enable(void)
{
	preempt_enable();
}
