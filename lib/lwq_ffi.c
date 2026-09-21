// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/lwq.rs: the exports of lib/lwq.c with their license, and
 * single-call wrappers for the C macros and inlines that the Rust code calls.
 */

#include <linux/export.h>
#include <linux/lwq.h>
#include <linux/spinlock.h>

EXPORT_SYMBOL_GPL(__lwq_dequeue);
EXPORT_SYMBOL_GPL(lwq_dequeue_all);

/*
 * lwq_empty() is a static inline of <linux/lwq.h>: the acquire load of
 * q->ready and llist_empty(&q->new).
 */
bool c_lwq_empty(struct lwq *q);
bool c_lwq_empty(struct lwq *q)
{
	return lwq_empty(q);
}

void c_lwq_spin_lock(struct lwq *q);
void c_lwq_spin_lock(struct lwq *q)
{
	spin_lock(&q->lock);
}

void c_lwq_spin_unlock(struct lwq *q);
void c_lwq_spin_unlock(struct lwq *q)
{
	spin_unlock(&q->lock);
}
