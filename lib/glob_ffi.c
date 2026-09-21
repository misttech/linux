// SPDX-License-Identifier: (GPL-2.0 OR MIT)
/*
 * C side of lib/glob.rs: the exports of lib/glob.c with their license, and its
 * module metadata. glob_match() and glob_match_len() call no C, so there are
 * no wrappers.
 */

#include <linux/module.h>
#include <linux/glob.h>
#include <linux/export.h>

MODULE_DESCRIPTION("glob(7) matching");
MODULE_LICENSE("Dual MIT/GPL");

EXPORT_SYMBOL(glob_match);
EXPORT_SYMBOL(glob_match_len);
