// SPDX-License-Identifier: GPL-2.0
/*
 * C side of lib/ctype.rs: the export of the _ctype table, with the license of
 * lib/ctype.c.
 */

#include <linux/ctype.h>
#include <linux/export.h>

EXPORT_SYMBOL(_ctype);
