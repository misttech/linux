// SPDX-License-Identifier: GPL-2.0
/*
 * Micro-benchmarks for timerqueue, run inside the kernel.
 *
 * linux-rust's userspace harness measures lib/timerqueue.c against
 * lib/timerqueue.rs by linking each into a userspace binary. That answers
 * what the code costs there, not what it costs here, and the kernel's own
 * ftrace profiler cannot answer it either: lib/ is built without the ftrace
 * hooks and Rust gets no fentry flag, so no ported symbol can be traced.
 *
 * This runs the same four loops as that harness, on the same 64 nodes with
 * the same batch size, against whichever implementation CONFIG_RUST_KERNEL
 * selected. Nothing here is instrumented and nothing is language specific:
 * it calls the exported API, so the two builds differ only in what is
 * behind timerqueue_add() and timerqueue_del().
 *
 * Output is one line per batch,
 *
 *	timerqueue_bench: <benchmark> <iteration> <ns per op>
 *
 * which is the format linux-rust's compare.py reads, so both measurements
 * are summarized by the same statistics.
 *
 * Boot with timerqueue_benchmark.run=1.
 */

#include <linux/init.h>
#include <linux/kernel.h>
#include <linux/ktime.h>
#include <linux/math64.h>
#include <linux/moduleparam.h>
#include <linux/printk.h>
#include <linux/timerqueue.h>

/* The harness constants, so a batch here and a batch there are the same. */
#define NR		64
#define OPS		(1UL << 14)
#define WARMUP_BATCHES	20

static bool run;
static unsigned int iterations = 10;
static unsigned int batches = 50;

module_param(run, bool, 0444);
MODULE_PARM_DESC(run, "run the benchmarks at late_initcall");
module_param(iterations, uint, 0444);
MODULE_PARM_DESC(iterations, "iterations, each a full sweep of the benchmarks");
module_param(batches, uint, 0444);
MODULE_PARM_DESC(batches, "measured batches per benchmark per iteration");

static struct timerqueue_head head;
static struct timerqueue_node nodes[NR];

/*
 * The harness runs one process per iteration, so every iteration starts on an
 * empty queue. Here the iterations share one address space, and bench_iterate
 * deliberately leaves the queue full, so the state is reset instead - once per
 * iteration, not per batch, which is what makes an iteration here the same
 * thing as one run of the harness binary.
 *
 * Resetting per batch would change what bench_iterate measures: its inserts
 * are guarded by timerqueue_node_queued(), so only the first batch pays for
 * them and the rest are pure walking. A reset in between would make every
 * batch pay 64 inserts, which is exactly where the two implementations are
 * reported to differ.
 */
static void bench_setup(void)
{
	int j;

	/*
	 * Before this runs the nodes are zero filled, and a zeroed node reports
	 * itself as queued: RB_EMPTY_NODE() tests __rb_parent_color against the
	 * node's own address. bench_reset() would then delete nodes that were
	 * never added, from a head that was never initialised.
	 */
	timerqueue_init_head(&head);
	for (j = 0; j < NR; j++)
		timerqueue_init(&nodes[j]);
}

static void bench_reset(void)
{
	int j;

	for (j = 0; j < NR; j++) {
		if (timerqueue_node_queued(&nodes[j]))
			timerqueue_del(&head, &nodes[j]);
		timerqueue_init(&nodes[j]);
	}
	timerqueue_init_head(&head);
}

/* Fill in increasing order, so every insert becomes the new rightmost. */
static void bench_add_del_sorted(unsigned long n)
{
	unsigned long i;
	int j;

	for (i = 0; i < n / NR; i++) {
		for (j = 0; j < NR; j++) {
			nodes[j].expires = j;
			timerqueue_add(&head, &nodes[j]);
		}
		for (j = 0; j < NR; j++)
			timerqueue_del(&head, &nodes[j]);
	}
}

/* Fill in decreasing order, so every insert becomes the new leftmost. */
static void bench_add_del_reverse(unsigned long n)
{
	unsigned long i;
	int j;

	for (i = 0; i < n / NR; i++) {
		for (j = 0; j < NR; j++) {
			nodes[j].expires = NR - j;
			timerqueue_add(&head, &nodes[j]);
		}
		for (j = 0; j < NR; j++)
			timerqueue_del(&head, &nodes[j]);
	}
}

/* An order that makes the tree rebalance both ways. */
static void bench_add_del_shuffled(unsigned long n)
{
	unsigned long i;
	int j;

	for (i = 0; i < n / NR; i++) {
		for (j = 0; j < NR; j++) {
			nodes[j].expires = (j * 37) % NR;
			timerqueue_add(&head, &nodes[j]);
		}
		for (j = 0; j < NR; j++)
			timerqueue_del(&head, &nodes[j]);
	}
}

/* Walk a full tree with the unit's iterator. */
static void bench_iterate(unsigned long n)
{
	unsigned long i;
	int j;

	for (j = 0; j < NR; j++) {
		nodes[j].expires = (j * 37) % NR;
		if (!timerqueue_node_queued(&nodes[j]))
			timerqueue_add(&head, &nodes[j]);
	}

	for (i = 0; i < n / NR; i++) {
		struct timerqueue_node *p = timerqueue_getnext(&head);

		while (p)
			p = timerqueue_iterate_next(p);
	}
}

static void bench_run_single(const char *name, void (*fn)(unsigned long),
			     unsigned int iteration)
{
	unsigned int b;

	for (b = 0; b < WARMUP_BATCHES; b++)
		fn(OPS);

	for (b = 0; b < batches; b++) {
		unsigned long flags;
		u64 start, elapsed, per_op;

		/*
		 * A batch is about a third of a millisecond, so interrupts stay
		 * off for it: a timer interrupt landing inside the timed region
		 * is worth more than the difference being measured.
		 */
		local_irq_save(flags);
		start = ktime_get_ns();
		fn(OPS);
		elapsed = ktime_get_ns() - start;
		local_irq_restore(flags);

		/* Four decimals of a nanosecond, as the harness prints. */
		per_op = div64_u64(elapsed * 10000, OPS);
		pr_info("timerqueue_bench: %s %u %llu.%04llu\n",
			name, iteration, per_op / 10000, per_op % 10000);
	}
}

static int __init timerqueue_benchmark_init(void)
{
	unsigned int i;

	if (!run)
		return 0;

	bench_setup();
	pr_info("timerqueue_bench: begin %u iterations of %u batches\n",
		iterations, batches);
	for (i = 1; i <= iterations; i++) {
		/* One iteration is one run of the harness binary. */
		bench_reset();
		bench_run_single("add_del_sorted", bench_add_del_sorted, i);
		bench_run_single("add_del_reverse", bench_add_del_reverse, i);
		bench_run_single("add_del_shuffled", bench_add_del_shuffled, i);
		bench_run_single("iterate", bench_iterate, i);
	}
	pr_info("timerqueue_bench: end\n");
	return 0;
}
late_initcall(timerqueue_benchmark_init);
