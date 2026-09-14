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

### Two repositories

- **This tree** holds the port: kernel code (ports, `rust/kr`, their
  Kconfig/Makefile wiring), this file with the porting rules, the porting
  skills, and the editor workspace. Every code commit is shaped like a
  kernel patch.
- **linux-rust** (`misttech/linux-rust`) holds the infrastructure: build,
  boot, test and benchmark scripts, the userspace harness, unit records, the
  test and benchmark skills, and its own `AGENTS.md`.
- The rules in this file bind work in both repositories.

### The work is agent-shaped

Porting is one mechanical transformation, applied a few hundred times:

1. Extract the C layout.
2. Generate a `#[repr(C)]` mirror plus const size/align/offset assertions.
3. Implement the exported symbols.
4. Classify every `unsafe` against the fixed taxonomy (below).
5. Generate a differential test against the C implementation.
6. Check the unit's `unsafe` budget.

Most of these steps can be checked by a machine, so agent output is
**certified by checks, not reviewed line by line**. The hypothesis under
test is that loop, not Rust and not Linux.

### Value case

- **Not performance.** Expect parity. arm64 carries a real regression risk:
  Rust cannot express LKMM dependency ordering, so `rcu_dereference` becomes
  `Ordering::Acquire`.
- **Not a direct CVE reduction.** Recent CVEs are in leaves (nvmem, ksmbd,
  nfsd, KVM), not in core.
- **The gain is second-order.** Most leaf bugs misuse a core primitive. Once
  `KRef<T>`, `RcuPtr<T>` and the branded intrusive list encode their
  invariants in types, leaves written against them cannot make those mistakes.
