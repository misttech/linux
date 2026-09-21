// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * C side of lib/plist.rs: single-call wrappers for the C inlines and macros that
 * the Rust code calls. lib/plist.c exports nothing, so there are no export lines.
 */

#include <linux/bug.h>
#include <linux/list.h>
#include <linux/plist.h>

void c_plist_list_add(struct list_head *new, struct list_head *head);
void c_plist_list_add(struct list_head *new, struct list_head *head)
{
	list_add(new, head);
}

void c_plist_list_add_tail(struct list_head *new, struct list_head *head);
void c_plist_list_add_tail(struct list_head *new, struct list_head *head)
{
	list_add_tail(new, head);
}

void c_plist_list_del_init(struct list_head *entry);
void c_plist_list_del_init(struct list_head *entry)
{
	list_del_init(entry);
}

void c_plist_warn_add_node_linked(void);
void c_plist_warn_add_node_linked(void)
{
	WARN_ON(1);
}

void c_plist_warn_add_prio_list_linked(void);
void c_plist_warn_add_prio_list_linked(void)
{
	WARN_ON(1);
}

/*
 * A Rust panic is not BUG(), and an extern function that never returns is one that
 * objtool does not know about, so the condition goes in.
 */
void c_plist_bug_on(bool condition);
void c_plist_bug_on(bool condition)
{
	BUG_ON(condition);
}

#ifdef CONFIG_DEBUG_PLIST
#include <linux/init.h>
#include <linux/ktime.h>
#include <linux/module.h>
#include <linux/printk.h>
#include <linux/sched/clock.h>

void c_plist_warn_prev_next(struct list_head *t, struct list_head *t_next,
			    struct list_head *t_prev, struct list_head *p,
			    struct list_head *p_next, struct list_head *p_prev,
			    struct list_head *n, struct list_head *n_next,
			    struct list_head *n_prev);
void c_plist_warn_prev_next(struct list_head *t, struct list_head *t_next,
			    struct list_head *t_prev, struct list_head *p,
			    struct list_head *p_next, struct list_head *p_prev,
			    struct list_head *n, struct list_head *n_next,
			    struct list_head *n_prev)
{
	WARN(1,
			"top: %p, n: %p, p: %p\n"
			"prev: %p, n: %p, p: %p\n"
			"next: %p, n: %p, p: %p\n",
			 t, t_next, t_prev,
			p, p_next, p_prev,
			n, n_next, n_prev);
}

unsigned int c_plist_local_clock(void);
unsigned int c_plist_local_clock(void)
{
	return local_clock();
}

long long c_plist_ktime_get(void);
long long c_plist_ktime_get(void)
{
	return ktime_get();
}

void c_plist_debug_start(void);
void c_plist_debug_start(void)
{
	printk(KERN_DEBUG "start plist test\n");
}

void c_plist_debug_end(void);
void c_plist_debug_end(void)
{
	printk(KERN_DEBUG "end plist test\n");
}

void c_plist_debug_elapsed(long long elapsed);
void c_plist_debug_elapsed(long long elapsed)
{
	pr_debug("plist_add worst case test time elapsed %lld\n", elapsed);
}

/* The test is in Rust; this is only its initcall. */
int plist_test_run(void);

static int __init c_plist_test(void)
{
	return plist_test_run();
}

module_init(c_plist_test);
#endif
