# Project: linux-mist

Port Linux's core to Rust: primitives first, then subsystems such as the
scheduler and MM. Agents do the work, and memory layout enforcement is the
correctness oracle.

Everything under "Project decisions" is settled. Do not reopen it in code,
reviews, or commit messages. Propose a change to this file instead.

## Project decisions

### Replacement, not bindings

A port **replaces** a C unit in place. It is not a binding in `rust/kernel/`.

- `lib/refcount.rs` sits next to `lib/refcount.c` and implements the same
  symbols with the same ABI.
- The existing header (`include/linux/refcount.h`) is the ABI contract and
  does not change. C callers are not touched.
- `CONFIG_RUST_KERNEL` selects C or Rust at build time, for every ported
  unit at once. C stays the reference for differential testing and rollback.
  Deleting the C file is a separate, later decision.
