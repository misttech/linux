// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/kfifo.rs: the exports of lib/kfifo.c with their license,
 * and wrappers for the macros and inlines the Rust code cannot call.
 *
 * struct scatterlist is deliberately not mirrored in Rust: it is
 * config-dependent (dma_length under CONFIG_NEED_SG_DMA_LENGTH, dma_flags
 * under CONFIG_NEED_SG_DMA_FLAGS), and setup_sgl() indexes an array of them.
 * The wrappers below take the index, so the Rust side only ever holds an
 * opaque pointer and never needs sizeof(struct scatterlist).
 */

#include <linux/bug.h>
#include <linux/dma-mapping.h>
#include <linux/export.h>
#include <linux/kfifo.h>
#include <linux/scatterlist.h>
#include <linux/slab.h>
#include <linux/uaccess.h>

EXPORT_SYMBOL(__kfifo_alloc_node);
EXPORT_SYMBOL(__kfifo_free);
EXPORT_SYMBOL(__kfifo_init);
EXPORT_SYMBOL(__kfifo_in);
EXPORT_SYMBOL(__kfifo_out);
EXPORT_SYMBOL(__kfifo_out_peek);
EXPORT_SYMBOL(__kfifo_out_linear);
EXPORT_SYMBOL(__kfifo_from_user);
EXPORT_SYMBOL(__kfifo_to_user);
EXPORT_SYMBOL(__kfifo_dma_in_prepare);
EXPORT_SYMBOL(__kfifo_dma_out_prepare);
EXPORT_SYMBOL(__kfifo_max_r);
EXPORT_SYMBOL(__kfifo_len_r);
EXPORT_SYMBOL(__kfifo_in_r);
EXPORT_SYMBOL(__kfifo_out_r);
EXPORT_SYMBOL(__kfifo_out_peek_r);
EXPORT_SYMBOL(__kfifo_out_linear_r);
EXPORT_SYMBOL(__kfifo_skip_r);
EXPORT_SYMBOL(__kfifo_from_user_r);
EXPORT_SYMBOL(__kfifo_to_user_r);
EXPORT_SYMBOL(__kfifo_dma_in_prepare_r);
EXPORT_SYMBOL(__kfifo_dma_in_finish_r);
EXPORT_SYMBOL(__kfifo_dma_out_prepare_r);

void *c_kfifo_kmalloc_array_node(size_t n, size_t size, gfp_t gfp, int node);
void *c_kfifo_kmalloc_array_node(size_t n, size_t size, gfp_t gfp, int node)
{
	return kmalloc_array_node(n, size, gfp, node);
}

unsigned long c_kfifo_copy_from_user(void *to, const void __user *from,
				     unsigned long n);
unsigned long c_kfifo_copy_from_user(void *to, const void __user *from,
				     unsigned long n)
{
	return copy_from_user(to, from, n);
}

unsigned long c_kfifo_copy_to_user(void __user *to, const void *from,
				   unsigned long n);
unsigned long c_kfifo_copy_to_user(void __user *to, const void *from,
				   unsigned long n)
{
	return copy_to_user(to, from, n);
}

void c_kfifo_sg_set_buf(struct scatterlist *sgl, unsigned int idx,
			const void *buf, unsigned int len);
void c_kfifo_sg_set_buf(struct scatterlist *sgl, unsigned int idx,
			const void *buf, unsigned int len)
{
	sg_set_buf(sgl + idx, buf, len);
}

void c_kfifo_sg_set_dma_address(struct scatterlist *sgl, unsigned int idx,
				dma_addr_t addr);
void c_kfifo_sg_set_dma_address(struct scatterlist *sgl, unsigned int idx,
				dma_addr_t addr)
{
	sg_dma_address(sgl + idx) = addr;
}

void c_kfifo_sg_set_dma_len(struct scatterlist *sgl, unsigned int idx,
			    unsigned int len);
void c_kfifo_sg_set_dma_len(struct scatterlist *sgl, unsigned int idx,
			    unsigned int len)
{
	sg_dma_len(sgl + idx) = len;
}

/*
 * BUG_ON() is a macro, and the record DMA paths use it to reject nents == 0.
 * Dropping it would silently change behaviour on a contract violation, and a
 * Rust panic is not BUG().
 */
void c_kfifo_bug_on(bool condition);
void c_kfifo_bug_on(bool condition)
{
	BUG_ON(condition);
}

/*
 * The barrier this unit is about: the data must be in the fifo before the
 * index that publishes it is incremented.
 */
void c_kfifo_smp_wmb(void);
void c_kfifo_smp_wmb(void)
{
	smp_wmb();
}
