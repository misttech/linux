// SPDX-License-Identifier: GPL-2.0-only
/*
 * C side of lib/uuid.rs: the exports of lib/uuid.c with their licenses. The
 * Rust code calls get_random_bytes() and hex_to_bin(), which are ordinary
 * exported functions, so there are no wrappers.
 */

#include <linux/export.h>
#include <linux/uuid.h>

EXPORT_SYMBOL(guid_null);
EXPORT_SYMBOL(uuid_null);
EXPORT_SYMBOL(generate_random_uuid);
EXPORT_SYMBOL(generate_random_guid);
EXPORT_SYMBOL_GPL(guid_gen);
EXPORT_SYMBOL_GPL(uuid_gen);
EXPORT_SYMBOL(uuid_is_valid);
EXPORT_SYMBOL(guid_parse);
EXPORT_SYMBOL(uuid_parse);
