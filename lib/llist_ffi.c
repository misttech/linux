// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/llist.rs: the exports of lib/llist.c with their license.
 */

#include <linux/export.h>
#include <linux/llist.h>

EXPORT_SYMBOL_GPL(llist_del_first);
EXPORT_SYMBOL_GPL(llist_del_first_this);
EXPORT_SYMBOL_GPL(llist_reverse_order);
