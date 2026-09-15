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

### Starting point: `refcount_t` / `kref`

This unit separates the bridge problem from the layout problem, and it forces
the highest-risk design decision first. `KRef<T>`:

- mirrors `kref`'s embedded-count layout exactly;
- is **never** `Arc`;
- **never** hands out `&mut T`;
- **never** reasons about uniqueness, because C may hold a reference Rust
  cannot see.

`rust/kernel/sync/refcount.rs` and `rust/kernel/sync/aref.rs` are bindings
over the C implementation. They are not the model for ports, and ports do
not change them.

### Testing needs no kernel build

- **Layout:** BTF from running distro kernels gives real layouts across real
  configs.
- **Memory ordering:** validated with `loom`.
- **Behavior:** a userspace harness in linux-rust builds the real
  `lib/refcount.c` and `lib/refcount.rs` against the real header, and tests
  them differentially.

### Kill criteria

Stop the project and report if any of these holds:

1. Layout cannot be pinned across configs, which makes the dual C/Rust model
   unsound.
2. `loom` cannot validate the ordering without contortions.
3. `KRef` needs `unsafe` at most call sites, so the abstraction buys nothing.

Every port, review, test and benchmark report states its status against these
three criteria.

## Where ports live

Paths are relative to this tree, except those marked *(linux-rust)*.

| File | Role |
|------|------|
| `<dir>/<unit>.c` | Existing C. Built only when `CONFIG_RUST_KERNEL=n` |
| `<dir>/<unit>.rs` | The Rust implementation. Exports the C symbols as `#[no_mangle] pub extern "C"` with exact C names and signatures. May also offer a typed Rust API (e.g. `KRef<T>`) |
| `<dir>/<unit>_ffi.c` | Only when needed; see [FFI](#ffi-unit_ffic) |
| `include/linux/<unit>.h` | The ABI contract. Unchanged |
| `main.rs` (tree root) | Crate root, built only when `CONFIG_RUST_KERNEL=y`. One `#[path = "<dir>/<unit>.rs"] pub mod <unit>;` per port |
| `harness/<unit>/` *(linux-rust)* | Userspace harness, loom models, benchmarks |
| `units/<unit>.md` *(linux-rust)* | Unit record: layout source, `unsafe` budget, test and benchmark results, kill-criteria status |

**Why one crate.** In `scripts/Makefile.build`, the `%.o: %.c` rule comes
before `%.o: %.rs`. A `<unit>.rs` next to `<unit>.c` therefore cannot build
as its own `<unit>.o`. All ports compile into one crate rooted at `main.rs`,
the way Zircon roots its kernel Rust at `zircon/kernel/main.rs`.

**Gate: `CONFIG_RUST_KERNEL`.** A single option switches every ported unit
together. Do not add per-unit options. It is defined in `init/Kconfig`,
right after `config RUST`, and every unit's Makefile uses the same pattern
(refcount example):

```make
# init/Kconfig
config RUST_KERNEL
	bool "Rust implementation of ported core kernel units"
	depends on RUST

# lib/Makefile (take refcount.o out of the obj-y list)
obj-$(if $(CONFIG_RUST_KERNEL),,y) += refcount.o
obj-$(CONFIG_RUST_KERNEL) += refcount_ffi.o
```

This option is unrelated to `CONFIG_RUST_KERNEL_DOCTESTS`.

Rules:
- **Dependencies.** A replacement depends only on `core`, `kr`, other ports
  in the `main.rs` crate, and its own `extern "C"` declarations. It never
  depends on the `kernel` or `bindings` crates. Those are bindings over the
  C being replaced, and the port must also build in the harness.
- **Exports.** Every exported symbol keeps its C name, signature and export
  license.
- **Header inlines.** `static inline` functions in the header stay in C.
  Moving one out of line into Rust changes codegen for every caller, so it
  needs `benchmark-vs-c` first. Header-only units such as `kref` start as a
  typed Rust API in `lib/<unit>.rs`, with any `<unit>_ffi.c` next to it,
  because `include/` has no build rules.

## `unsafe` taxonomy

Every `unsafe` block is tagged `// SAFETY: (Un) <invariant, naming the C
contract>`. An `unsafe` that fits no class is rejected, not argued for.

| Tag | Class | Justified by |
|-----|-------|--------------|
| U1 | FFI call | A documented C function contract |
| U2 | Layout cast (C ↔ `#[repr(C)]` mirror) | Const layout assertions in the same unit |
| U3 | Raw deref | A named C invariant (lock held, pointer pinned) |
| U4 | Refcount lifetime | A held `kref`/`refcount_t` reference |
| U5 | RCU access | An RCU read-side critical section in scope |
| U6 | `Send`/`Sync` impl | The C locking rules for the type |
| U7 | In-place init of C-owned memory | Pinning plus an init contract |

`unsafe fn` needs a `# Safety` section. Every unit records its `unsafe` budget
(a count per tag). Code that *uses* an abstraction has a budget of 0.

## Common kernel lib: `kr`

`rust/kr/` is the zero-dependency core crate (only `core`), modeled on
Zircon's `zr`. Replacements and the harness share it, so every unit
compiles in both places without changes. Nothing may be added to `kr` that
depends on `kernel`, `bindings` or another in-tree crate.

| Use | For |
|-----|-----|
| `kr::static_assert_layout!(T, size = S, align = A, field @ OFF, ...)` | **Every** `#[repr(C)]` mirror: exact size, align, and every field offset, with values taken from BTF |
| `kr::static_assert!` | Other compile-time facts |
| `kr::Opaque<T>`, `kr::OpaqueBytes<N>` | Fields that are unported or config-dependent |
| `kr::defer` | Scope-exit cleanup |

Size is asserted as equal, never `<=`, because the mirror *is* the C type.

## FFI: `<unit>_ffi.c`

Create one only when needed, next to the unit, the way Zircon uses
`*_ffi.cc`. It holds two kinds of declarative content and nothing else:

| Content | Why | Form |
|---------|-----|------|
| Export lines for symbols implemented in Rust | `rust/exports.c` exports every Rust symbol as GPL-only, but the port must keep the original license | `EXPORT_SYMBOL(refcount_dec_if_one);`, copied from `<unit>.c` |
| Wrappers for C inlines and macros the Rust code calls (`WARN_ONCE`, `mutex_lock`, ...) | Rust cannot call these directly, and it must not use `bindings` | `c_<unit>_<fn>`. The prototype sits above the definition in the same file (`-Wmissing-prototypes`), and `<unit>.rs` declares it in `extern "C"` |

Rules:
- **No logic.** A wrapper is a single forwarding call.
- **Build it** with `obj-$(CONFIG_RUST_KERNEL) += <unit>_ffi.o`.
- **Single export path.** The crate's symbols must never also go through
  `rust/exports.c`, or they are exported twice.
- **Calls into `c_*` functions are `unsafe` class U1.** The harness
  provides userspace stubs for them.

## One-time wiring

Done. It needs linux-rust's `scripts/make.sh LLVM=1 rustavailable` to pass.

1. `config RUST_KERNEL` is in `init/Kconfig`, after `config RUST`.
2. `rust/Makefile` builds `rust/kr/` and the `main.rs` crate as objects, the
   way it builds `ffi.o`, with `@include/generated/rustc_cfg` in the flags.
   The `main.rs` crate is built only under `CONFIG_RUST_KERNEL`.
3. Both crates are registered in `scripts/generate_rust_analyzer.py`.

## Workflow

Each skill is one role, and each hands off to the next:

| Skill | Lives in | Role | Writes code |
|-------|----------|------|-------------|
| `port-c-to-rust` | this tree | Replace one C unit | Yes |
| `review-rust-port` | this tree | Certify a port against this file | No |
| `test-rust-port` | linux-rust | Layout, loom, differential, KUnit | Tests only |
| `benchmark-vs-c` | linux-rust | Performance vs C, x86_64 and arm64 | Benchmarks only |

Rules for every role:

- Work on real in-tree files. A report is not an implementation.
- A step counts as done only when `git status` shows it.
- A review is invalid if the diff is empty.

### Worktrees and branches

**Agents work in a git worktree, never in the primary checkout.** Worktrees live
in `.worktree/` at the repo root — ignored, so an agent's tree is never a source
of stray untracked files in someone else's `git status`. One agent, one
worktree, one branch: a second agent editing the same working tree turns two
independent changes into one unreviewable diff, and a rebase under a running
build breaks the tree out from under it.

```bash
git worktree add .worktree/<name> -b <agent>/<model>/<feature|fix>/<name>
git worktree remove .worktree/<name>    # when the branch has landed
```

Every branch is named `<agent>/<model>/<feature|fix>/<name>` — who ran it, what
model, what kind of change, and what it touches. The prefix is what makes agent
work attributable after the fact; a bare `fix-parser` says nothing about where
it came from.

| segment | value |
|---|---|
| `<agent>` | the agent that did the work — `claude`, `codex`, `cursor` |
| `<model>` | the model behind it — `opus-5`, `gpt-5`, `grok-4-5` |
| `<feature\|fix>` | `feature` or `fix` |
| `<name>` | kebab-case subject, matching the worktree directory |

### Branch model

Project decisions:
- **`linux-rust` is the work branch.** Agent branches start from it and
  land back on it.
- **`master` stays in sync with upstream** and carries no project commits.
- **Update by rebasing, never merging.** Rebase `linux-rust` onto `master`,
  then rebase any open agent branches onto the new `linux-rust`.

```bash
git fetch origin master:master    # fast-forward master to upstream
git rebase master linux-rust      # replay the work on top
```

## Building

Project decision: this tree is never built in place, because an in-tree
`.config` breaks out-of-tree builds. Build, boot, test and benchmark through
linux-rust's `scripts/`. Its `AGENTS.md` lists the commands and the
environment variables that set every location.

Build every port twice, with `CONFIG_RUST_KERNEL=n` and `=y`. Kernel builds
take minutes, so give them long timeouts. Prefer the build-free tests in the
harness while iterating.

## Style and contributions

- Follow `Documentation/rust/coding-guidelines.rst` and
  `Documentation/process/coding-style.rst`. Match local style over "best
  practice".
- Follow `Documentation/process/coding-assistants.rst`: **never add
  `Signed-off-by`**. Add `Assisted-by: LLM [tools]` instead.
- All code is GPL-2.0-only and every file carries an SPDX identifier.
- Keep the in-body comments of `<unit>.c` in `<unit>.rs`, renaming
  identifiers as needed.
- Do not add dependencies to kernel code. Tooling in linux-rust may add them
  when the reason is stated.
- Do not glob the whole tree. Scope searches to the directories involved.

## Commits

Project decision: commits in this tree use kernel style.

- Subject `subsystem: summary`, imperative, lower case after the prefix.
- Body wrapped at 72 columns, explaining the reason and intention.
- `Assisted-by:` trailer, never `Signed-off-by` (see above).
- linux-rust uses its own scheme; see its `AGENTS.md`.

## References

Fuchsia checkout: `/home/bherrera/Projects/misttech/fuchsia-mist`

- `zircon/kernel/main.rs`: a single crate root, with `#[path]` modules that
  sit beside the C++ they replace.
- `zircon/kernel/platform/timer_ffi.cc`: the shape of an `_ffi` file.
- `zircon/skills/cpp-to-rust-rubric/SKILL.md`: layout parity, FFI, comment
  parity and pitfalls.
- `zircon/skills/cpp-to-rust/SKILL.md`: the coder/reviewer orchestration loop
  and the git verification gate.
- `zircon/skills/fbl-intrusive-porting/SKILL.md`: intrusive containers and
  refcounting.
- `src/lib/zr/`: the model for `kr`.

## Updating this file

If agents keep making the same mistake, or a decision changes, propose an edit
here. Do not add workarounds in code.
