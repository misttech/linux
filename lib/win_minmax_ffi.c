// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/win_minmax.rs: the exports of lib/win_minmax.c with their
 * license.
 */

#include <linux/export.h>
#include <linux/win_minmax.h>

EXPORT_SYMBOL(minmax_running_max);
EXPORT_SYMBOL(minmax_running_min);
