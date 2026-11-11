// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/rcu_ptr.rs: single-call wrappers for the two RCU inlines that the Rust
 * code calls. synchronize_rcu() and call_rcu() are functions, which the Rust code declares
 * itself. include/linux/rcupdate.h is header-only for this unit, so there is no C file that
 * this replaces and no export line.
 */

#include <linux/rcupdate.h>

void c_rcu_read_lock(void);
void c_rcu_read_lock(void)
{
	rcu_read_lock();
}

void c_rcu_read_unlock(void);
void c_rcu_read_unlock(void)
{
	rcu_read_unlock();
}
