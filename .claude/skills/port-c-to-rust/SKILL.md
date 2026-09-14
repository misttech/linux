---
name: port-c-to-rust
description: Replace one Linux C unit (e.g. lib/refcount.c) with a Rust implementation beside it (lib/refcount.rs) - same symbols, same ABI, same header, gated by CONFIG_RUST_KERNEL - using BTF layout mirrors, kr assertions, unsafe classification, a differential test and an unsafe budget. Use when asked to port, convert, or rewrite a kernel C file in Rust.
---

# Port C to Rust

Project decision: a port **replaces** the C unit in place; it is not a
binding under `rust/kernel/`. Read `AGENTS.md` first. Its placement, FFI
rules and `unsafe` taxonomy are binding.

## Input

A C unit: `<dir>/<unit>.c`, its header, and its exported symbols.

## Steps

1. **Record the unit.** In linux-rust's `units/<unit>.md`, list:
   - the C file and header;
   - every exported symbol, with its signature and export license
     (`EXPORT_SYMBOL` or `_GPL`);
   - the `static inline` functions that stay in C;
   - locking and RCU rules;
   - caller patterns that matter for the design.
2. **Extract the layout.** Run `pahole -C <type> /sys/kernel/btf/vmlinux`
   and check at least one other config. Record config-driven differences. If
   the layout cannot be pinned, **stop: kill criterion 1**.
3. **Mirror the types** in `<dir>/<unit>.rs`: `#[repr(C)]`, followed by
   `kr::static_assert_layout!` listing every field. Fields that are unported
   or config-dependent use `kr::Opaque<T>` or `kr::OpaqueBytes<N>`.
4. **Implement the exports** in `<dir>/<unit>.rs` as
   `#[no_mangle] pub extern "C" fn <c_name>`, with the exact C signature.
   - Keep the semantics exactly: saturation, `WARN`, return values.
   - Keep the in-body comments from `<unit>.c`.
   - Never hand out `&mut T` from a shared object, and never assume
     uniqueness.
   - Take `&T` only where the C contract guarantees non-null.
5. **FFI, only if needed.** Create `<dir>/<unit>_ffi.c` holding:
   - the `EXPORT_SYMBOL*` lines copied from `<unit>.c`;
   - `c_<unit>_<fn>` wrappers for C inlines and macros the Rust code calls,
     each with its prototype above the definition.

   Declare the wrappers in `<unit>.rs` in an `extern "C"` block. No logic.
6. **Wire the gate.** `CONFIG_RUST_KERNEL` is the only switch. Do not add a
   per-unit option.
   - In the directory's Makefile, build `<unit>.o` only when the gate is off
     (`obj-$(if $(CONFIG_RUST_KERNEL),,y)`), and `<unit>_ffi.o` only when it
     is on (`obj-$(CONFIG_RUST_KERNEL)`).
   - In `main.rs`, add `#[path = "<dir>/<unit>.rs"] mod <unit>;`.
7. **Classify each `unsafe`** as U1–U7. If one fits no class, redesign.
8. **Differential test.** Add the unit to the linux-rust harness (see
   `test-rust-port` there).
9. **Check the budget.** Write the count per tag into the unit record. If
   most call sites need `unsafe`, **stop: kill criterion 3**.
10. **Build both ways.**
    - Run linux-rust's `scripts/build.sh LLVM=1` (output in
      `$KBUILD_OUTPUT`) with `CONFIG_RUST_KERNEL=n` and again with `=y`.
    - Run `CLIPPY=1` and `rustfmtcheck`. Leave no warnings.
    - `Module.symvers` must list the same symbols and export types in both
      builds.

## Rules

- Depend only on `core`, `kr` and the unit's own `extern "C"`
  declarations. Never on `kernel` or `bindings`.
- Do not change the header. Moving a `static inline` out of line requires
  `benchmark-vs-c` first.
- `rcu_dereference`-style loads use `Ordering::Acquire`. Record which LKMM
  ordering is lost.
- Edit real files. Add an SPDX `GPL-2.0` header to new files.

## Output

- The changed paths and `git status`.
- The results of both builds.
- The `Module.symvers` diff between the C and Rust builds.
- The `unsafe` budget table.
- The status of kill criteria 1–3.

Then hand off to `review-rust-port`.
