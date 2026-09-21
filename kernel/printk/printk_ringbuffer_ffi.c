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

long c_printk_ringbuffer_load(const atomic_long_t *p);
long c_printk_ringbuffer_load(const atomic_long_t *p)
{
	return atomic_long_read(p);
}

long c_printk_ringbuffer_load_acquire(const atomic_long_t *p);
long c_printk_ringbuffer_load_acquire(const atomic_long_t *p)
{
	return atomic_long_read_acquire(p);
}

void c_printk_ringbuffer_store(atomic_long_t *p, long value);
void c_printk_ringbuffer_store(atomic_long_t *p, long value)
{
	atomic_long_set(p, value);
}

bool c_printk_ringbuffer_cas(atomic_long_t *p, long *old, long value);
bool c_printk_ringbuffer_cas(atomic_long_t *p, long *old, long value)
{
	return atomic_long_try_cmpxchg(p, old, value);
}

bool c_printk_ringbuffer_cas_relaxed(atomic_long_t *p, long *old, long value);
bool c_printk_ringbuffer_cas_relaxed(atomic_long_t *p, long *old, long value)
{
	return atomic_long_try_cmpxchg_relaxed(p, old, value);
}

bool c_printk_ringbuffer_cas_release(atomic_long_t *p, long *old, long value);
bool c_printk_ringbuffer_cas_release(atomic_long_t *p, long *old, long value)
{
	return atomic_long_try_cmpxchg_release(p, old, value);
}

void c_printk_ringbuffer_inc(atomic_long_t *p);
void c_printk_ringbuffer_inc(atomic_long_t *p)
{
	atomic_long_inc(p);
}

void c_printk_ringbuffer_rmb(void);
void c_printk_ringbuffer_rmb(void)
{
	smp_rmb();
}

void c_printk_ringbuffer_irq_save(unsigned long *flags);
void c_printk_ringbuffer_irq_save(unsigned long *flags)
{
	local_irq_save(*flags);
}

void c_printk_ringbuffer_irq_restore(unsigned long flags);
void c_printk_ringbuffer_irq_restore(unsigned long flags)
{
	local_irq_restore(flags);
}

void *c_printk_ringbuffer_copy(void *dst, const void *src, size_t n);
void *c_printk_ringbuffer_copy(void *dst, const void *src, size_t n)
{
	return memcpy(dst, src, n);
}

void *c_printk_ringbuffer_clear(void *dst, int value, size_t n);
void *c_printk_ringbuffer_clear(void *dst, int value, size_t n)
{
	return memset(dst, value, n);
}

u8 c_printk_ringbuffer_read_u8(const u8 *src);
u8 c_printk_ringbuffer_read_u8(const u8 *src)
{
	return READ_ONCE(*src);
}

u16 c_printk_ringbuffer_read_u16(const u16 *src);
u16 c_printk_ringbuffer_read_u16(const u16 *src)
{
	return READ_ONCE(*src);
}

u32 c_printk_ringbuffer_read_u32(const u32 *src);
u32 c_printk_ringbuffer_read_u32(const u32 *src)
{
	return READ_ONCE(*src);
}

u64 c_printk_ringbuffer_read_u64(const u64 *src);
u64 c_printk_ringbuffer_read_u64(const u64 *src)
{
	return READ_ONCE(*src);
}

bool c_printk_ringbuffer_panic_cpu(void);
bool c_printk_ringbuffer_panic_cpu(void)
{
	return panic_on_this_cpu();
}

bool c_printk_ringbuffer_warn_descriptor_state(bool condition);
bool c_printk_ringbuffer_warn_descriptor_state(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_descriptor_reserve(bool condition);
bool c_printk_ringbuffer_warn_descriptor_reserve(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_allocation_size(bool condition);
bool c_printk_ringbuffer_warn_allocation_size(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_reallocation_size(bool condition);
bool c_printk_ringbuffer_warn_reallocation_size(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_block_wrap(bool condition);
bool c_printk_ringbuffer_warn_block_wrap(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_block_begin_alignment(bool condition);
bool c_printk_ringbuffer_warn_block_begin_alignment(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_block_next_alignment(bool condition);
bool c_printk_ringbuffer_warn_block_next_alignment(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_block_minimum_size(bool condition);
bool c_printk_ringbuffer_warn_block_minimum_size(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_block_maximum_size(bool condition);
bool c_printk_ringbuffer_warn_block_maximum_size(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_empty_text_length(bool condition);
bool c_printk_ringbuffer_warn_empty_text_length(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_text_length(bool condition);
bool c_printk_ringbuffer_warn_text_length(bool condition)
{
	return WARN_ON_ONCE(condition);
}

bool c_printk_ringbuffer_warn_commit(bool condition);
bool c_printk_ringbuffer_warn_commit(bool condition)
{
	return WARN_ON_ONCE(condition);
}

void c_printk_ringbuffer_warn_len_zero(unsigned short length);
void c_printk_ringbuffer_warn_len_zero(unsigned short length)
{
	pr_warn_once("wrong text_len value (%hu, expecting 0)\n", length);
}

void c_printk_ringbuffer_warn_len_max(unsigned short length, unsigned int max);
void c_printk_ringbuffer_warn_len_max(unsigned short length, unsigned int max)
{
	pr_warn_once("wrong text_len value (%hu, expecting <=%u)\n", length, max);
}
