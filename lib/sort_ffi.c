// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/sort.rs: the exports of lib/sort.c with their license, and a
 * single-call wrapper for the cond_resched() macro that the Rust code calls.
 */

#include <linux/export.h>
#include <linux/sched.h>
#include <linux/sort.h>

EXPORT_SYMBOL(sort_r);
EXPORT_SYMBOL(sort_r_nonatomic);
EXPORT_SYMBOL(sort);
EXPORT_SYMBOL(sort_nonatomic);

void c_sort_cond_resched(void);
void c_sort_cond_resched(void)
{
	cond_resched();
}
