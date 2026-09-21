---
name: port-intrusive-containers
description: Port a Linux intrusive container (list_head, hlist, llist, rb_node/rb_root, plist, klist) to a typed Rust API over the same C layout - node state inside the object, C inlines keeping working on the same memory, ownership expressed by a trait, safe operations through cursors or a brand, and differential and loom tests against the real header. Use with port-c-to-rust when the unit is a container that objects embed, or when asked to add a typed Rust layer over list_head, hlist or rbtree. Adapted from Fuchsia's fbl-intrusive-porting.
---

# Port intrusive containers

Project decision: a port shares memory with C, it does not wrap it. Read
`AGENTS.md` first. Its placement, FFI rules and `unsafe` taxonomy bind this skill,
and `port-c-to-rust` covers the mechanics it does not repeat (layout mirrors, the
`_ffi.c` file, the gate, both builds). This skill is what an *intrusive container*
adds.

This is adapted from Fuchsia's `fbl-intrusive-porting` skill
(`zircon/skills/fbl-intrusive-porting/SKILL.md` in fuchsia-mist), which records what
was learned porting `SinglyLinkedList`. Its patterns carry over where the problem is
the same, and are replaced where Linux differs; see the table below.

The reference implementation is `lib/llist.rs`: `HasLlistNode`, `ListItem`,
`LList<P>` with its `Producer` and `Consumer`, `Drain<P>`, and the `AtomicLink`
trait that lets a loom model run the port's own code. `KRef<T>` in `lib/kref.rs` is
the reference for an owning pointer that never hands out `&mut T`. Copy their
shape before inventing one.

## What is different from Fuchsia

The FBL containers are C++ templates that own or borrow their elements. Linux
containers are structures that *objects embed*, and the container never owns
anything. That changes the design.

| | Fuchsia `fbl` | Here |
|---|---|---|
| Who owns elements | the container, through `UniquePtr`, `RefPtr` or a raw pointer | nobody in C. Ownership is by convention, so the Rust layer has to state it |
| The other language | C++ shares containers with Rust | **C keeps using the same list**: `list_add()`, `list_for_each_entry()` and the rest are `static inline` in the header, and they stay in C |
| Owning pointer types | defined next to the container | a port has only `core`, `kr` and other ports. It cannot allocate or use `Box`, so ownership is a trait that callers implement |
| Node lookup | `#[derive(Containable)]` and attributes | no proc macros: an `unsafe trait` with an `OFFSET` from `offset_of!`, one per container kind and tag |
| Layout source | C++ `static_assert` against the Rust one | BTF or `pahole`, then `kr::static_assert_layout!` for every field; config-dependent facts from the C compiler |
| Sentinels | a private value in C++ | the C's own: a self-linked `list_head`, `LIST_POISON1` and `LIST_POISON2`, `pprev` |
| Test oracle | Rust macro-generated tests per pointer type | the real header in the linux-rust harness, `LIST_KUNIT_TEST`, loom, and mutants |
| Failure checks | `debug_assert!` in `Drop` | no panic; a C `WARN` wrapper where the C would warn |

## Core principles

1. **Same memory, same invariants.** A node is the C struct, laid out exactly. C
   code and Rust code may both be operating on the same list, and Rust cannot see the
   C references. So the Rust layer never reasons about who else is on the list, never
   hands out `&mut` to a node or to an object that embeds one, and never assumes
   uniqueness. This is the `KRef<T>` rule applied to membership.
2. **Layout parity, measured.** Take size, alignment and every field offset from BTF,
   and assert them all. Do not hand-compute.

   | Type | Size on 64-bit | Fields |
   |---|---|---|
   | `list_head` | 16 | `next`@0, `prev`@8 |
   | `hlist_head` | 8 | `first`@0 |
   | `hlist_node` | 16 | `next`@0, `pprev`@8 |
   | `llist_node`, `llist_head` | 8 | one pointer each |
   | `rb_node` | 24, aligned to `sizeof(long)` | `__rb_parent_color`@0, `rb_right`@8, `rb_left`@16 |
   | `rb_root` | 8 | `rb_node`@0 |
   | `rb_root_cached` | 16 | `rb_root`@0, `rb_leftmost`@8 |
   | `plist_head` / `plist_node` | 16 / 40 | list heads inside |

   Check 32-bit from DWARF, as `lib/llist.rs` does. `rb_node` packs the parent
   pointer and the colour into one word, so its "field" is a tagged value: the
   mirror needs accessors that keep the tag bits, and the differential test has to
   compare that word, not only the tree shape.
3. **Intrusive.** The object holds the node, and the container holds nothing that
   the C container does not.
4. **Ownership is a trait, not a type.** A port cannot allocate. Model the owner as
   an `unsafe trait ListItem` with `into_raw()` and `from_raw()`, as `llist.rs` does,
   and implement it in the port only for what the port owns (`KRef<T>`). A leaf crate
   that has `KBox` or `Arc` implements it for those. The trait's `# Safety` says that
   between the two calls the object is valid and nothing but a list touches its node.

## Patterns

### 1. Interior mutability, and never `&mut`

Nodes are mutated through shared references, and C mutates them behind Rust's back.
A `&mut Node` to a shared object is undefined behaviour.

- Use `AtomicPtr` for a lock-free container (`llist_node.next` is one), and `Relaxed`
  for what C does with `READ_ONCE()` and `WRITE_ONCE()`, as the other ports do.
- Use `kr::Opaque` or `UnsafeCell` for a container that a lock protects, and touch
  it only where the lock is held.
- Record any LKMM ordering the mapping loses in the unit record.

### 2. A list head that points at itself cannot be moved

An empty `list_head` points at itself, and an `hlist_node`'s `pprev` points into its
predecessor. Rust moves values, so a head that has been initialized and is then moved
dangles. `llist` does not have this problem, which is why it was easy.

- Hold the head `Pin`ned, or as a type that is never moved once built, and initialize
  it in place: that is `INIT_LIST_HEAD()` on pinned memory, class **U7**.
- Say in the `# Safety` of the constructor who guarantees the memory stays put.
- The differential test must build a list, then `memcpy` it, then walk it from C, to
  show what a move does.
- An empty `hlist_head` is one NULL pointer, so it is `const`-initializable and may move
  until something points into it: no in-place init and no U7 for it, and `HList::new()`
  is `const`. Pin at the first link. A list that is used through a `Pin<Box<[HList]>>`
  needs no unsafe; a head **embedded in a structure** needs a pin projection to reach it,
  and a **static** head has no safe path at all. Count those per container in the
  client (see the client test), and say so under kill criterion 3.

### 3. Node location: `Has*Node<Tag>`

One object often sits on several lists of the same kind (`task_struct` has more than
one `list_head`). A single `HasListNode` trait per kind cannot say which field.

- `unsafe trait HasListNode<Tag> { const OFFSET: usize }`, one impl per field, with a
  zero-sized `Tag` type per list. This replaces Fuchsia's `#[sll_node]` and
  `#[dll_node]` attributes, and goes further, because the kind alone is not enough.
- Get `OFFSET` from `core::mem::offset_of!`, and write the `entry_of` and `node_of`
  helpers once (`container_of()`), as `llist.rs` does.
- A `macro_rules!` in `kr` may generate the impl. Nothing in `kr` may depend on
  `kernel`, `bindings` or another crate.

### 4. Safe and unsafe APIs

- Owned operations take a `P: ListItem` and are safe: `push_back(p)`, `pop_front()`.
- Operations on a raw `NonNull<T>` are `_raw` and `unsafe`, with a `# Safety` that
  names the C invariant (lock held, node on this list, object live).
- Tag every block `// SAFETY: (Un)`. The raw deref of a node is U3, `Send` and `Sync`
  impls are U6, in-place init is U7. Code that *uses* the container has a budget of 0.
- **Refuse a node that is already linked, and claim it atomically.** A second reference
  to an object can offer it to a list its node is on, and linking a linked node leaves
  that list pointing at it, at freed memory once the owner drops. C has no defence, and
  a safe API needs one. Return the item on refusal. If two lists can have two locks, the
  check and the mark must be one compare-exchange on a field of the node (`pprev`, or
  `next`), from "free" to a marker: a check followed by a store lets both through. Put
  the compare-exchange behind a one-method trait so that loom runs the port's own
  function, and add a threaded test. Also give one field one tag: a marker trait
  implemented by the macro, with the field's offset as a const parameter, makes a second
  impl for the same field a coherence error.
- **Derive the pointers a list stores from the object, not from a reference to its
  node.** A pointer from `&Node` has the provenance of the node's bytes, and stepping
  back by `OFFSET` to the object then uses it beyond them. The stores look right and
  every test passes. Carry `NonNull<Node>` from `node_of(obj)` end to end and make a
  reference only to load and store, and run Miri (below).
- **Unsafe that is not a dereference.** A pin projection (`Pin::new_unchecked`,
  `get_unchecked_mut`) fits no class of the taxonomy: U3 is a raw dereference and U7 is
  in-place init. Tag it U7 with a comment that says so until `CLAUDE.md` gets a class
  for it (proposal: U8, pin projection), and never argue it into U3. Keep them out of
  `Drop` by giving `Drop` a helper that takes no `Pin`.
- Keep the unsafe in a handful of private node helpers (`get_next`, `set_next`,
  `node_of`, `entry_of`), so the container methods read as safe code and the
  invariants are in one place to audit.

### 5. Positional operations: a cursor, and a brand

`insert_after`, `erase_next` and `list_move()` take a position. With raw pointers the
compiler cannot know that the node is on *this* list, which is the bug class the
project exists to remove.

- Give the list a `CursorMut` that borrows it mutably, so the borrow checker stops a
  second mutation while a cursor exists.
- Then add the **brand**: `CLAUDE.md` calls the target a *branded* intrusive list. A
  node reference carries an invariant lifetime `'brand` that only one list can hand
  out, so `list.remove(node)` accepts only nodes of that list. Pick a mechanism
  (a `with_list(|list| ...)` closure, or a token as in GhostCell), write down what it
  costs a caller in ergonomics, and prototype it in the harness before wiring it in.
- Removal by node without the head (`hlist_del(&obj->node)`) is the operation kernel
  code uses most on an hlist, and only a brand can offer it safely. Beware that "is on
  this list" is not stable: a `Copy` node reference goes stale on removal, and the
  object is freed. A sound brand needs a **linear** handle, returned once by the push
  and consumed by the removal, and it is no help against C that unlinks the node. If
  the port ships without it, say what a caller does instead (a cursor walk), and that
  the operation is missing.
- A node still reachable from C is not covered by any brand. State that in the
  unit record as the limit of the guarantee.

### 6. Iteration

C's `list_for_each_entry_safe()` exists because deleting during iteration is a bug.
Do not offer a safe iterator that yields `&mut T`. Offer an iterator that yields
`&T` (the object mutates through interior mutability), and offer removal through the
cursor, where the "current" node is known. A `Drain<P>` that yields owned `P`s and
reclaims the rest on drop is the model (`llist.rs`).

### 7. Concurrency modes

Decide which the container is, and test that mode:

- **Caller holds a lock.** Either the API takes a witness for it, since a port cannot use
  `kernel`'s lock guards (a reference to a small trait the caller implements), or, as
  `hlist` does, `Pin<&mut List>` is the exclusivity, and the unsafe `from_raw()` that views
  a C-owned head carries the lock obligation in its `# Safety`. The second is simpler and
  costs no parameter; say which you chose. What a lock does not cover is a node shared
  between two lists under two locks (the claim above).
- **RCU** (`list_add_rcu()`, `list_for_each_entry_rcu()`). Publication is a `Release`
  store, the read is `Acquire`, and the dependency ordering C gets for free is lost:
  say so in the record and measure it on arm64.
- **Lock-free** (`llist`). Make the atomics a trait (`AtomicLink`) so that a loom model
  runs the port's own code.

### 8. Hardening and poison are part of the ABI

- With `CONFIG_DEBUG_LIST` or `CONFIG_LIST_HARDENED`, the C inlines call
  `__list_add_valid_or_report()` and `__list_del_entry_valid_or_report()`, which
  `lib/list_debug.c` exports. A Rust `add` and `del` that skip them lose the
  corruption reports. Either call them through `c_*` wrappers under the same
  configuration, or port `list_debug.c` first.
- `list_del()` writes `LIST_POISON1` and `LIST_POISON2`
  (`0x100` and `0x122` plus `POISON_POINTER_DELTA`, which depends on
  `CONFIG_ILLEGAL_POINTER_VALUE`). C code and crash dumps rely on them. A Rust `del`
  must write the same values, generated from the C compiler the way the layout
  constants are (`kernel/port-layout.c`).
- `list_del_init()` and `list_del()` leave different memory behind, and C can tell.
  Port both, and compare the memory of a removed node in the differential test.

### 9. No `Drop` panic

Fuchsia asserts in `Drop` that a node has left its container. The kernel does not
panic on this. For an object the port owns, drain or unlink in `Drop` (as `LList`
does). For a C-owned object there is no `Drop` to run, and the check is the C one:
`list_empty()` on the node, or the poison. Where the C would `WARN`, call it through
a single-call `c_<unit>_warn_*` wrapper, class U1.

### 10. Cached fields must be kept up

`rb_root_cached.rb_leftmost` is a cache that C maintains. A Rust `insert` or `erase`
that does not update it corrupts every C reader. Fuchsia's size-tracker underflow is
the same class of bug. List every cached field of the container in the unit record,
and make the differential test compare it after every operation.

## Testing

Certify with checks. A test that only compares the order of elements is not enough.

1. **Layout**: `kr::static_assert_layout!` for every field, and the numbers from BTF or
   DWARF (see the table above). Config-dependent constants are generated.
2. **Differential, on shared memory.** The linux-rust harness builds the real
   `include/linux/list.h` (or `rbtree.h`) and drives one list from both sides: Rust
   `push_back` then C `list_for_each_entry()` and `list_del()`, and the reverse. After
   every operation compare the **memory of every node and of the head**, not only the
   traversal: a Rust `del` that forgets the poison or the cache passes an order check.
   This replaces Fuchsia's `allocated_in_rust` flag, because the interop is the thing
   under test. The harness allocates with `malloc`, where the kernel uses `kmalloc`.
   Run the real header in the harness, not a copy: the stubs chain with `#include_next`
   (`types.h`, `compiler.h`), guard what libc also defines (`__always_inline`), and stub
   the context-analysis annotations (`__context_unsafe`). **Drive the two sides in a
   mixed mode**: an implementation chosen at random for each operation, whose trace must
   equal the pure C trace. And check the invariants of the C memory in the driver itself
   (links point back, lists end, nodes are where a model says, off-list nodes are NULL or
   poisoned), so a port that is wrong the same way in both modes still fails.
3. **Loom** for any container with atomics or an RCU publication, through the trait
   seam, as `harness/llist/loom` does in linux-rust. Assert what must hold (nothing
   lost, nothing seen twice), not what one interleaving happens to do: the first
   two-dequeuer model of `lwq` asserted more than the C guarantees, and loom refuted it.
   **Races.** Two threads that must meet need a spin rendezvous, not a futex barrier: the
   barrier's wake-up skew hid a check-then-act claim in most runs. Loom finds it in every
   run, which is why the claim goes behind a seam.
4. **Miri.** A stand-in for the owner (`KRef` becomes a plain pointer) lets Miri build the
   port without C. Run every operation against a model, under Stacked Borrows and Tree
   Borrows. It finds provenance errors nothing else does, and it failed the first
   `hlist` for the reason above. Miri is a rustup component: `rustup component add
   --toolchain nightly miri rust-src`.
5. **KUnit**: `LIST_KUNIT_TEST` (`lib/tests/list-test.c`) tests the C inlines, and
   `hashtable_test.c` the hlist ones. They are the oracle for the C side of the shared
   list. They do not exercise the Rust layer, so they do not replace item 2.
6. **Mutants.** Mutate the port and confirm each test fails: swap `next` and `prev`,
   skip the poison, skip the cache, leave a node linked after `del`. Record the
   survivors and the equivalent ones.
7. **The client test that measures kill criterion 3.** Write a small client that uses the
   container the way a leaf would, and count its `unsafe`. The budget is **0**. If a
   client needs `unsafe` at most call sites the abstraction buys nothing, which is
   the reason the project would stop.
   Count by section, mechanically (per type, per container, call sites), and fail the
   build if the call sites have any. Add compile-fail tests, one per misuse the types are
   meant to stop (mutate while iterating, use after push, push to an unpinned list, move
   a pinned list, write through an iterator, two cursors, two tags on a field), each with
   its error code, next to a control that compiles.
8. Test with each `ListItem` owner the harness can build (raw, `KRef<T>`, and `Box`,
   since the harness has `std`), with a macro that generates a module per owner, as
   Fuchsia does. Use stack-allocated objects for the raw tests, which cannot leak.

## Step by step

1. **Record the container.** `units/<unit>.md` in linux-rust: the header, the C
   inlines that stay, every configuration option that changes behaviour, the locking
   or RCU rule, who uses it (`git grep`, with a whole-word pattern and your own files
   excluded).
2. **Mirror the nodes** with `#[repr(C)]`, interior mutability, and
   `static_assert_layout!` on every field. Generate poison and config facts.
3. **Write the traits**: `Has*Node<Tag>`, `ListItem`, and the atomic or lock seam.
4. **Implement**: owned safe operations, then `_raw`, then the cursor and brand, then
   iteration and `Drain`. Keep the C comments. Keep the unsafe in the node helpers.
5. **Hardening**: the `c_*` wrappers for the debug validators and any `WARN`, or a
   dependency on the `list_debug.c` port.
6. **Wire it**: a header-only unit is a typed API in `lib/<unit>.rs`, added to
   `main.rs` as a `#[path]` module, with any `_ffi.c` next to it. Do not change
   `include/`, and do not move a `static inline` out of line: that needs
   `benchmark-vs-c` first.
7. **Test** as above, build both `CONFIG_RUST_KERNEL` ways, run clippy and rustfmt, and
   compare `vmlinux.symvers` from non-clippy builds.
8. **Hand off** to `review-rust-port`, and state kill criteria 1 to 3 in every report.

## Order

Start with the containers where the layout is settled and the invariants are small,
and build up to the ones with a cache or a lock.

1. `llist`: done, and the model for this skill.
2. `hlist_head` and `hlist_node`: one pointer of head, and `pprev` to get right. Hash
   tables are built on it, and `hashtable_test.c` is a C-side oracle.
3. `list_head`: by far the most used (about 5,300 files use it or `list_add()`), and
   the one where the brand pays off. The 50 `static inline` functions of `list.h`
   stay in C.
4. `rb_node` and `rb_root`: the parent and colour word, and the cached leftmost.
5. `plist`, `klist`: built on lists and locks, so last.

## Pitfalls

- A moved self-linked head, which corrupts silently (pattern 2).
- A differential test that compares order and not node memory (poison, caches, the
  tagged `rb_node` word).
- A `Drop` or an iterator that panics or asserts, where the kernel must not.
- Assuming membership: C may have added or removed the node.
- A safe API that hands out `&mut T`.
- Porting a `static inline` out of line without a benchmark.
- Missing the debug validators, so a hardened kernel loses its corruption reports.
- Keying the brand or the offset trait on the container kind alone, when one object sits
  on several lists of that kind.
- Generated poison: `DEFINE()` prints a signed number, so `0xdead000000000000` arrives
  as `-2401263026318606080`, and a `usize` constant cannot take it. Emit a negative value
  as `(N_isize) as usize`.
- A `macro_rules!` macro that the crate does not use is an `unused_macros` warning in a
  kernel build: `#[macro_export]` it.
- `KRef::get(&T)` in `lib/kref.rs` makes an owner from any reference, a stack object's
  included, so a list of `KRef`s can end up pointing at a dead frame. Say so in the
  record; it weakens the U4 argument.
- Mutation runs that swap the source under one build directory: cargo trusts mtimes, so
  a real file older than the mutant's is not rebuilt. `touch` it before the real run.
- A generic function over the list's tag needs `Obj: HasHlistNode<T>` written out; the
  associated type's bound is not implied when the type is fixed by `Target = Obj`.
- `pahole -C <type> vmlinux` on a `CONFIG_RUST_KERNEL=y` kernel returns the Rust
  mirror. Read the C type from an `_ffi.o`.

## Output

- The changed paths and `git status`.
- The layout table with its source, and the generated constants.
- The `unsafe` budget: the port's, and the client's, which must be 0.
- The differential, loom, KUnit and mutant results, including what survived.
- The status of kill criteria 1 to 3, with the client count for 3.

Then hand off to `review-rust-port`.
