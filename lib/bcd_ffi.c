// SPDX-License-Identifier: GPL-2.0
/* Export definitions for the Rust BCD replacement. */

#include <linux/bcd.h>
#include <linux/export.h>

EXPORT_SYMBOL(_bcd2bin);
EXPORT_SYMBOL(_bin2bcd);
