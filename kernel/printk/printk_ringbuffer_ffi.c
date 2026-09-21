// SPDX-License-Identifier: GPL-2.0-only
/* LKMM primitives used by the Rust printk ring buffer. */
#include <linux/atomic.h>
#include <linux/irqflags.h>
#include <linux/string.h>
#include <linux/bug.h>
#include "internal.h"
#include "printk_ringbuffer.h"
#include <kunit/visibility.h>

EXPORT_SYMBOL_IF_KUNIT(prb_reserve);
EXPORT_SYMBOL_IF_KUNIT(prb_commit);
EXPORT_SYMBOL_IF_KUNIT(prb_read_valid);
EXPORT_SYMBOL_IF_KUNIT(prb_init);

long c_prb_load(const atomic_long_t *p);
long c_prb_load(const atomic_long_t *p)
{
	return atomic_long_read(p);
}

long c_prb_load_acquire(const atomic_long_t *p);
long c_prb_load_acquire(const atomic_long_t *p)
{
	return atomic_long_read_acquire(p);
}

void c_prb_store(atomic_long_t *p, long value);
void c_prb_store(atomic_long_t *p, long value)
{
	atomic_long_set(p, value);
}

bool c_prb_cas(atomic_long_t *p, long *old, long value);
bool c_prb_cas(atomic_long_t *p, long *old, long value)
{
	return atomic_long_try_cmpxchg(p, old, value);
}

bool c_prb_cas_relaxed(atomic_long_t *p, long *old, long value);
bool c_prb_cas_relaxed(atomic_long_t *p, long *old, long value)
{
	return atomic_long_try_cmpxchg_relaxed(p, old, value);
}

bool c_prb_cas_release(atomic_long_t *p, long *old, long value);
bool c_prb_cas_release(atomic_long_t *p, long *old, long value)
{
	return atomic_long_try_cmpxchg_release(p, old, value);
}

void c_prb_inc(atomic_long_t *p);
void c_prb_inc(atomic_long_t *p)
{
	atomic_long_inc(p);
}

void c_prb_rmb(void);
void c_prb_rmb(void)
{
	smp_rmb();
}

void c_prb_irq_save(unsigned long *flags);
void c_prb_irq_save(unsigned long *flags)
{
	local_irq_save(*flags);
}

void c_prb_irq_restore(unsigned long flags);
void c_prb_irq_restore(unsigned long flags)
{
	local_irq_restore(flags);
}

void * c_prb_copy(void *dst, const void *src, size_t n);
void * c_prb_copy(void *dst, const void *src, size_t n)
{
	return memcpy(dst, src, n);
}

void * c_prb_clear(void *dst, int value, size_t n);
void * c_prb_clear(void *dst, int value, size_t n)
{
	return memset(dst, value, n);
}

void * c_prb_find(const void *src, int value, size_t n);
void * c_prb_find(const void *src, int value, size_t n)
{
	return memchr(src, value, n);
}

bool c_prb_panic_cpu(void);
bool c_prb_panic_cpu(void)
{
	return panic_on_this_cpu();
}

bool c_prb_warn_0(bool condition);
bool c_prb_warn_0(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_1(bool condition);
bool c_prb_warn_1(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_2(bool condition);
bool c_prb_warn_2(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_3(bool condition);
bool c_prb_warn_3(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_4(bool condition);
bool c_prb_warn_4(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_5(bool condition);
bool c_prb_warn_5(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_6(bool condition);
bool c_prb_warn_6(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_7(bool condition);
bool c_prb_warn_7(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_8(bool condition);
bool c_prb_warn_8(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_9(bool condition);
bool c_prb_warn_9(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_10(bool condition);
bool c_prb_warn_10(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_prb_warn_11(bool condition);
bool c_prb_warn_11(bool condition)
{
	return WARN_ON_ONCE(condition);
}

void c_prb_warn_len_zero(unsigned short length);
void c_prb_warn_len_zero(unsigned short length)
{
	pr_warn_once("wrong text_len value (%hu, expecting 0)\n", length);
}

void c_prb_warn_len_max(unsigned short length, unsigned int max);
void c_prb_warn_len_max(unsigned short length, unsigned int max)
{
	pr_warn_once("wrong text_len value (%hu, expecting <=%u)\n", length, max);
}
