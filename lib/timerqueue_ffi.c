// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * C side of lib/timerqueue.rs: the exports of lib/timerqueue.c with their
 * license, and one wrapper per WARN_ON_ONCE() site, so that each keeps its
 * own once flag.
 *
 * The rbtree operations the port calls are exported symbols already
 * (rb_insert_color, rb_erase, rb_erase_linked, rb_next), so they need no
 * wrapper. The inline helpers around them - rb_link_node(),
 * rb_insert_color_cached(), rb_erase_cached(), rb_first_cached() and
 * rb_link_linked_node() - are pointer manipulation that lib/timerqueue.rs
 * does itself through its mirrors.
 */

#include <linux/bug.h>
#include <linux/export.h>
#include <linux/timerqueue.h>

EXPORT_SYMBOL_GPL(timerqueue_add);
EXPORT_SYMBOL_GPL(timerqueue_del);
EXPORT_SYMBOL_GPL(timerqueue_iterate_next);
EXPORT_SYMBOL_GPL(timerqueue_linked_add);

void c_timerqueue_warn_add_queued(void);
void c_timerqueue_warn_add_queued(void)
{
	WARN_ON_ONCE(1);
}

void c_timerqueue_warn_del_not_queued(void);
void c_timerqueue_warn_del_not_queued(void)
{
	WARN_ON_ONCE(1);
}
