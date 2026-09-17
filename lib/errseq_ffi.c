// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/errseq.rs: the exports of lib/errseq.c with their license,
 * and the WARN() on an out-of-range error.
 */

#include <linux/bug.h>
#include <linux/types.h>
#include <linux/errseq.h>
#include <linux/export.h>

EXPORT_SYMBOL(errseq_set);
EXPORT_SYMBOL(errseq_sample);
EXPORT_SYMBOL(errseq_check);
EXPORT_SYMBOL(errseq_check_and_advance);

void c_errseq_warn_bad_err(int err);
void c_errseq_warn_bad_err(int err)
{
	WARN(1, "err = %d\n", err);
}
