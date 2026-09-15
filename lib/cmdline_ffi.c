// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/cmdline.rs: the exports of lib/cmdline.c with their license,
 * and a single-call wrapper for the isspace() macro that the Rust code calls.
 */

#include <linux/ctype.h>
#include <linux/export.h>
#include <linux/string.h>

EXPORT_SYMBOL(get_option);
EXPORT_SYMBOL(get_options);
EXPORT_SYMBOL(memparse);
EXPORT_SYMBOL(next_arg);

bool c_cmdline_isspace(char c);
bool c_cmdline_isspace(char c)
{
	return isspace(c);
}
