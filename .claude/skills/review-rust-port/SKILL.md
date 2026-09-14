---
name: review-rust-port
description: Certify a Rust replacement of a Linux C unit against AGENTS.md - ABI and export parity with the C file, placement beside the C source, kr layout assertions, unsafe taxonomy and budget, semantic parity, KRef invariants, FFI shims, comment parity. Read-only. Use when asked to review, audit, or certify a Rust port.
---

# Review Rust port

Project decision: review certifies the port with checks; it does not
re-review line by line what a check already proves. Read-only: never edit
files. Read `AGENTS.md` first.

## Gate

Run `git status` and `git diff`. If no in-tree files changed, reject at once
with "no implementation". Never review a plan or a report.

## Checklist

Each item is PASS or FAIL, with evidence given as `file:line` or a command's
output.

1. **Placement**
   - The port is `<dir>/<unit>.rs`, next to `<dir>/<unit>.c`, not under
     `rust/kernel/`.
   - It is wired through a `main.rs` `#[path]` and gated only by
     `CONFIG_RUST_KERNEL`. Any per-unit option fails.
   - The Makefile builds exactly one of `<unit>.o` or `<unit>_ffi.o`.
2. **ABI**
   - Every C exported symbol exists in Rust as `#[no_mangle] extern "C"`,
     with an identical signature.
   - `Module.symvers` is identical between the `=n` and `=y` builds,
     including export type.
   - `git diff include/` is empty.
3. **Dependencies.** The unit uses only `core`, `kr` and its own
   `extern "C"`. No `kernel::` or `bindings::`.
4. **Layout**
   - Every mirror has a `kr::static_assert_layout!` with exact size, align
     and **every** field, matching `pahole` for each config in the record.
   - Hand-written asserts and `<=` size checks fail.
5. **`unsafe`**
   - Every block carries `SAFETY(Un)`, and the tag is correct.
   - The justification names the actual C contract.
   - The counts match the recorded budget.
   - Code that uses the abstraction has 0 `unsafe`.
6. **Semantics**
   - The behavior of each C function is preserved: saturation, `WARN`,
     error and return paths.
   - Every behavior is covered by the differential test.
7. **Ownership invariants**
   - No `&mut T` from shared objects.
   - No uniqueness reasoning.
   - Nothing like `Arc` stands in for an embedded count.
   - Invisible C references are accounted for.
8. **Concurrency**
   - Atomic orderings are justified.
   - `Acquire` is used where LKMM ordering is lost, and the loss is recorded.
   - A loom model covers the ordering.
9. **FFI**
   - A `<unit>_ffi.c` exists only if needed.
   - It contains only `EXPORT_SYMBOL*` lines, whose license matches the C
     file, and single-call `c_<unit>_*` wrappers, each with a prototype.
   - The crate's symbols do not go through `rust/exports.c`.
10. **Comments.** The in-body comments of `<unit>.c` are kept in
    `<unit>.rs`.
11. **Build hygiene**
    - Clippy is clean.
    - `rustfmtcheck` is clean.
    - SPDX headers are present.
    - There is no `Signed-off-by`.
12. **Kill criteria.** State the status of 1–3 with evidence.

## Output

```
Verdict: APPROVE | CHANGES REQUIRED | KILL (criterion N)
Checklist: 1..12 PASS/FAIL + evidence
Required changes: numbered, actionable, file:line
```

Hand the required changes back to `port-c-to-rust`.
