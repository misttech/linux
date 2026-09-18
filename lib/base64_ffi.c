// SPDX-License-Identifier: GPL-2.0
/* Export definitions for the Rust base64 replacement. */

#include <linux/export.h>
#include <linux/base64.h>

EXPORT_SYMBOL_GPL(base64_encode);
EXPORT_SYMBOL_GPL(base64_decode);
