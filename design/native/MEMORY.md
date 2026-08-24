# Memory

`design/TODO.md#the-native-backend` states the problem and offers two answers:
"the language has no mutation and no destructors, so native either ships a GC or
does escape analysis with an arena per `Alloc` scope."

Both are wrong, and the reasons are specific enough to be worth writing down
before saying what is right instead. The answer is **non-atomic reference
counting with static elision and in-place reuse**, over a size-class allocator,
with `Alloc` as a *defined* accounting model rather than a measurement.

## 1. What the language gives us

Four properties, and every decision below is downstream of them.

- **No mutation.** A value's fields are written once, at construction. There is
  no assignment, no interior mutability, no `&mut`.
- **No destructors.** Freeing is the implementation's business entirely; nothing
  in a program can observe when it happens or run code at that point.
- **No threads.** The language has no concurrency construct, `core/effect` grants no
  effect that produces one, and nothing in the standard library spawns anything.
- **Effect-carrying values cannot be captured.** SPEC 10.6, checked. So a closure's
  environment is plain data.

## 2. Immutability implies acyclicity, and that is the whole argument

> **Lemma.** In a Buri program, the points-to graph over heap values is acyclic.

A value's fields are set once, at construction, from expressions evaluated
before the construction. So every reference in a value points to a value that
already existed, and "already existed" is a strict partial order. A cycle would
need a value pointing at something constructed after it, which requires either
assignment after construction (there is none) or a recursive binding whose
right-hand side is a value referring to the binding itself.

The second one is the hazard, so it is worth checking rather than asserting. It
cannot happen:

- **Recursive *types* are not recursive *values*.** `enum Rose { Node([Rose]) }`
  is fine; every `Rose` is built from `Rose`s that already exist. The recursion
  is in the type, and a type is not a heap object.
- **Recursive *functions* are not heap values.** Recursion goes through a
  top-level `fn`, which the middle end resolves to a code pointer
  (`monomorphize.rs`, `Callee::Func`). A code pointer is not reference counted.
- **A lambda cannot refer to itself.** `ExprKind::Lambda` captures a list of
  already-bound locals (`typed.rs:194`); there is no `let rec`, and a `let f = fn(x) => f(x)`
  fails name resolution because `f` is not in scope in its own initialiser.
- **A context cannot close a cycle.** SPEC 10.6 again: nothing effect-carrying
  is ever captured, so a context never ends up inside a value that the context
  also reaches.

That lemma is what makes reference counting **sound and complete** here: sound
because dropping to zero means unreachable, complete because there is no cycle
left over that a collector would have to find. Refcounting in a language with
mutation is a memory leak with extra steps; in this language it is a complete
collector.

It is worth defending the lemma with a test rather than only with an argument.
`cli/tests/native/memory.rs` runs a corpus of the shapes that would break it — a
recursive enum, a rose tree, a closure over a closure, a context stored in a
struct — and asserts that every allocation is freed at exit. A future language
feature that introduces a cycle will fail there rather than in production.

There is no "runtime built with leak checking on" to arrange: `buri_rt_alloc`
and `buri_rt_free` keep the four counters `buri_rt_heap_stats` reports, always,
because a relaxed add beside a `malloc` is not a cost anybody can measure and a
diagnostic that only exists in a special build is a diagnostic nobody runs. An
immortal block is removed from the live count when it is marked
(`buri_rt_make_immortal`), so a leak check does not report every string literal.
`cli/tests/native/runtime.rs` already asserts the property on a corpus of one,
from C; `cli/tests/native/memory.rs` is the same assertion over Buri programs.

## 3. Why not a tracing GC

Direct conflict with `design/LLVM-tips.md:2`.

A tracing collector has to find the roots, which means knowing which stack slots
and which registers hold pointers at every point a collection can happen. There
are two ways to have that in LLVM, and both are excluded:

- **Precise, via `gc.statepoint`.** Every GC-reachable pointer has to live in a
  stack slot the runtime can enumerate at a safepoint, which is `alloca` +
  reload around every call. That is exactly the `alloca` form
  `design/LLVM-tips.md:2` says not to generate, and running `mem2reg` over it
  does not help — the whole point of a statepoint is that the value is *not* in a
  register across the call. The instruction and the collector are incompatible,
  and the instruction is the one with a reason behind it.
- **Conservative, scanning the stack for anything that looks like a pointer.**
  This language's most common heap value is `[Int]` — an array of arbitrary
  64-bit integers, any of which may be numerically equal to a live address.
  Conservative scanning turns "a program computed a large number" into "a program
  retains an arbitrary allocation", and the retention is data-dependent and
  irreproducible. In a toolchain whose central claim is that two builds of a tree
  agree byte for byte, an irreproducible heap is the wrong kind of nondeterminism
  to introduce.

There is a third cost that applies to both: a collector has to be told about
every pointer the *runtime* holds too, which means 203 runtime functions each
growing a rooting discipline.

## 4. Why not an arena per `Alloc` scope

This is the one `design/TODO.md#the-native-backend` hopes for, and the honest
answer is that the effect system does not carry the information it would need.

**`Alloc` says a function allocates. It does not say when the allocation dies.**

An arena needs a scope: a point at which everything allocated since some earlier
point becomes unreachable, all at once. Look for one:

- `Alloc` is a **bound on a context** (`effect.buri:18-20`), and a bound propagates.
  `list.map` is `<C: Alloc>`, so every caller of `map` is `Alloc`-bounded, and so
  is every caller of *those*. In any program that maps a list, `main` is
  `Alloc`-bounded and the "Alloc scope" is the program.
- `Region` is a **value** (`effect.buri:16`: `export struct Region(export I64)`),
  returned by `allocate` and freely storable in a struct, returnable from a
  function, and placeable in a list. It is not a scope and it does not nest.
- There is no `with`, no `using`, no scoped-context expression. A context is
  built by `context { ... }` and lives as long as anything referring to it.

So an arena hung on the `Alloc` bound is an arena that is entered at `main` and
left at exit, which is the never-free strategy under a different name.

Escape analysis would help — a value that provably does not outlive its frame can
be stack-allocated — and it is worth having *as an optimization on top of*
something that is correct without it. It is not a strategy on its own, because
the cases it cannot prove need an answer, and "leak it" is not one.

### 4.1 And why not never-free

Viable for a compiler or a `grep`; not viable here. `core/http` is in the
standard library, `core/net` grants `fetch`, and a server that never frees is not
a server. `buri test --watch` (BUILD-AND-WATCH.md §4) is a long-lived process too.
Choosing a memory strategy that only works for programs that exit quickly is
choosing which programs the language is for, and nothing else in this language
has made that choice.

## 5. The decision: reference counting, elided and reusing

### 5.1 The counts

Header per VALUE-MODEL.md §2: 16 bytes at `ptr - 16`, `{ rc: u64, cap: u64 }`,
`rc == u64::MAX` meaning `IMMORTAL`.

```
incref(p):                          decref(p):
  if p == null: return                if p == null: return
  rc = load p[-16]                    rc = load p[-16]
  store p[-16] = saturating_add rc 1  if rc == IMMORTAL: return
                                      if rc == 1: drop_T(p); free(p); return
                                      store p[-16] = rc - 1
```

`incref` is branchless after the null test — a load, a saturating add (one `add`
plus one `cmov` on x86-64, one `adds`+`csinv` on aarch64), a store. `IMMORTAL`
stays `IMMORTAL` under saturation, which is what makes the immortal case free on
the increment side, where the traffic is.

`decref` has two branches, both well predicted: `IMMORTAL` is taken always or
never per call site in practice, and `rc == 1` is the uncommon case in shared
data and the common case in linear data — which is what §5.3 exploits.

Both are **open-coded by both backends**, never called. A call per reference
operation is the single reason reference counting has a reputation, and both
Cranelift and LLVM inline these to a handful of instructions with no spills. The
`drop_T` in the cold path *is* a call, to a generated per-type function.

Null checks: eliminated wherever the layout says the pointer is non-null, which is
everywhere except a niche-encoded `Option` (VALUE-MODEL.md §6). LLVM gets this for
free from `nonnull` (CODEGEN-LLVM.md §3).

### 5.2 Elision, which is where the cost goes

Naive reference counting increments on every parameter pass and decrements on
every scope exit, and it is slow. The fix is the one Koka's Perceus and Lean 4's
runtime both use, and this language is a better fit for it than either, because
it has no mutation at all. The algorithm's full details are in the paper —
Reinking, Xie, de Moura and Leijen, *Perceus: Garbage Free Reference Counting
with Reuse*, linked from
[../../reference/README.md](../../reference/README.md).

`middle::rc` computes, per parameter, whether the callee **owns** or **borrows**
it:

- A parameter is **borrowed** if the callee neither stores it in a constructed
  value, nor returns it, nor passes it to a function that owns it. A borrowed
  parameter needs no increment at the call and no decrement in the callee: the
  caller's own reference keeps it alive for the whole call.
- A parameter is **owned** otherwise, and the caller transfers a count.

The analysis is a fixpoint over the call graph, which is exact
(`monomorphize.rs:8-10`) — so, unlike in a language with dynamic dispatch, the
answer is a fact and not a conservative approximation. `xs.fold(f, init)`,
`xs.any(pred)`, `s.startsWith(p)`, `s.indexOf(n)`, `xs.len()`: every pure,
non-constructing operation in the standard library borrows everything, and
therefore touches no reference count at all.

On top of that, three local rules:

- **Drop the increment/decrement pair around a value that is dead immediately
  after.** The last use of a local transfers rather than copies.
- **`IMMORTAL` at compile time.** A literal, an interned constant aggregate
  (`generate.rs:457`'s `intern` moves to the middle end and applies to both
  backends), and any zero-sized value get no reference operations emitted at all,
  because the compiler knows they are immortal — the runtime `IMMORTAL` check
  is for values that reached a generic path.
- **Stack allocation for non-escaping aggregates.** The escape analysis §4
  declined to build a strategy on is a fine optimization: a struct constructed and
  consumed within one function, never stored and never returned, becomes an
  `alloca` (LLVM) / stack slot (Cranelift) with no header and no counts. This is
  the one place `alloca` is emitted, and it is emitted for a value that is never
  reloaded through a pointer, so `design/LLVM-tips.md:2`'s instruction is not
  violated — see CODEGEN-LLVM.md §2.3.

### 5.3 Reuse, which is where the copying goes

The other half of Perceus, and the reason an immutable language can be fast.

When a value is uniquely owned — `rc == 1` — nothing in the program can tell the
difference between building a new value and writing into the old one. That is the
whole of it, and everything below is which operations take the opportunity.

Reuse is guarded by a runtime `rc == 1` test, which is one compare against a
header word the operation was going to load anyway. When the test fails, the
fallback is allocate-and-copy, which is what would have happened unconditionally
without the feature.

#### What has landed, and where each fast path lives

The blocks in this implementation are not where a reader of the paragraph above
would guess, so the list is worth having explicitly. A struct, a tuple, an enum
and a closure record are **register or stack** values — `MakeStruct` is a
Cranelift stack slot and an LLVM aggregate — and the only counted heap blocks
are a `Str`'s bytes, a `[T]`'s elements, a closure *environment*, and the box a
recursive field goes behind (VALUE-MODEL.md §5.2). So the two operations worth
optimizing are the two that build the first two, and both are done:

- **`[T]` append — `cli/runtime/list.rs`'s `append_dest`, behind `list.push`
  and `list.concat`.** Both are runtime calls on both backends
  (`cranelift/runtime.rs`, `llvm/runtime.rs`), so the fast path is in the
  runtime and is shared. Three paths: *in place* when the block is uniquely
  owned and `cap >= (len + n) * stride`, writing past the end and taking one
  more reference; *grown* when it is unique and out of capacity, allocating
  `max(needed * 2, 64)` so the next append is in place; *exact* otherwise. A
  loop of `n` pushes therefore allocates O(log n) times, which is where
  VALUE-MODEL.md §4.1's amortized O(1) comes from.
- **`Str` concatenation — `cranelift/helpers.rs`'s `concat` and
  `llvm/emit.rs`'s `concat`.** `str.concat` is open-coded by both backends
  rather than called, so the fast path is written twice and pinned twice. The
  same three paths, with the capacity test allowing for a view that starts
  inside its block: `(ptr - base) + alen + blen <= cap`. A template of *k*
  holes, or a fold that concatenates, is the shape this turns from O(k)
  allocations into O(log k).

**Why the in-place write is unobservable.** `rc == 1` means exactly one live
value refers to the block. Every operation that produces a *new* view of a block
increfs its base before answering (`cli/runtime/text.rs`), so a second view would
be a second count; and static elision (§5.2) never duplicates a reference without
an `incref` — a borrowed parameter *aliases* the caller's reference rather than
adding one. So the aliases elision leaves behind are copies of that one value,
carrying the same `ptr` and the same `len`, and a write that starts at `ptr + len`
is invisible to all of them. The correctness of §5.2's counting is what licenses
§5.3's mutation, and there is no separate argument to make.

Which means a *wrong* count is not merely a leak or a use-after-free here: it is
a licence to overwrite something live. Writing the fast paths turned up two
places where `middle::rc` got the count wrong, both fixed with the code above
and both with a regression test beside them, and they are worth naming because
they are the shape the next one will have. A local scrutinized by **two**
consuming `match`es was dropped by each of them, because the first one erased it
from the liveness the second computed. And a **borrowed local handed to a
construct beside a sibling holding its last mention** — `f(s, g(s))`, or
`"${s} … ${s.len()}"` — was dropped after the sibling, while the construct was
still holding uncounted words copied out of it. Neither was visible to the
balance checker, which counts operations rather than orders them; both were
visible the moment an allocation reused the freed block.

**Growth policy: doubling with a floor of 64 bytes**, applied only when the left
operand is uniquely owned. A shared operand is not the one being built, so it
gets an exact allocation and no speculative capacity. The floor is
`layout::GROWTH_FLOOR` for the backends and `BURI_RT_GROWTH_FLOOR` for the
runtime — two constants for two crates that never link against each other, and a
disagreement between them costs a reallocation rather than an answer.

#### What is excluded, and why

- **A counted element type — `[Str]`, `[(Str, Int)]` — takes neither the
  in-place path nor the over-allocation.** Two reasons, and both are
  correctness rather than caution. Writing at index `len` would drop whatever
  reference that slot already held without a `decref`, because a slot past the
  end of one descriptor may hold an element a *longer*, now-dead descriptor put
  there. And the generated release glue for a `[T]` block walks **`cap /
  stride`** elements (`cranelift/helpers.rs`'s `release_elems`,
  `llvm/emit.rs`'s `Job::ReleaseElems`), so spare capacity would have the drop
  walk slots nothing ever wrote. Lifting it means adding a per-element
  *release* glue beside the `retain` this ABI already passes, and making that
  walk follow the element count rather than the capacity. That is the growth
  path, and it is a change in both backends rather than in the runtime.
- **Aggregate-cell reuse — the `S { ..old, field: new }` that Perceus is
  famous for — has no cell to reuse.** `middle::rc` computes the pairing
  (`FuncPlan::reuse`, behind `Options::reuse`, on by default) and it is correct
  as an analysis, but the construction it would rewrite does not allocate:

  ```buri ignore why="illustrative"
  match xs {
    .Cons(h, t) => .Cons(f(h), t),
    .Nil => .Nil,
  }
  ```

  compiles to a stack slot and a store, not to a heap cell and a `decref`.
  `cli/tests/native/cranelift.rs`'s
  `a_struct_update_loop_allocates_nothing_per_iteration` is the measurement
  rather than the claim: a thousand struct updates allocate exactly as many
  blocks as ten. Emitting the conditional form — a `rc == 1` branch, a write,
  and an allocate-and-`decref` on the other side — would be two backends' worth
  of new IR to save an allocation that is not happening. The pairing therefore
  stays analysis, and the *test* that would notice a layout change promoting
  aggregates to the heap is the thing that had to exist.
- **Cross-block reuse** — pairing a dying value with a construction in a
  different basic block — is a known extension and is not in v1. So is reuse
  across a function boundary.
- **Same-layout reuse across types** — writing a `Point` into a dying `Pair`'s
  cell because both are two words — is excluded for the same reason as the row
  above it plus one of its own: it would make the reuse decision depend on the
  layout table agreeing about two unrelated types, and a layout change would
  then silently change which programs mutate.

**The sanctioned direction, named so nobody has to re-derive it.** The
performance story for this design is Roc's, and Roc is the existence proof that
it works for a pure language shipped to users rather than only in a paper: Roc is
Perceus-style reference counted, with no tracing collector, and its speed comes
from **opportunistic in-place mutation at refcount 1** — mutate when there is a
single owner, share persistently otherwise — with as much of the ownership
decided statically as the compiler can manage, so the runtime `rc == 1` test is
skipped where the answer is already known. Koka calls the paradigm this enables
*functional but in-place*: an algorithm written as a pure fold that compiles to
the in-place loop its mutable twin would have been.

That is what §5.3's reuse *is*, and saying so pins the direction of every future
optimization here. The order of work is: more static ownership (fewer `rc == 1`
tests, not faster ones), then cross-block reuse, then reuse across a function
boundary. What is explicitly **not** on the path is adding a tracing collector
beside the counts to catch what they miss (§3 gives the reason), or making the
counts atomic before the language has threads (§5.4).

- Reinking, Xie, de Moura and Leijen, *Perceus: Garbage Free Reference Counting
  with Reuse*, PLDI 2021 — the algorithm, and the FBIP framing.
- Roc's implementation, and Teeuwissen's *Reference Counting with Reuse in Roc*
  (Utrecht, 2023) — the same algorithm in a shipped compiler, including where
  the reuse analysis pays and where it does not.

### 5.4 The allocator underneath

Single-threaded, no locks, no atomics.

**v1 is `malloc`-backed and has no size classes.** `buri_rt_alloc(payload)` is
one allocation of `16 + payload` bytes at 16-byte alignment with the header
written, and `buri_rt_free` returns it — a call, not an open-coded sequence. The
rest of this section is the growth path, and separating the two is deliberate
rather than a shortcut: everything *observable* about allocation is settled
either way. The header is the same 16 bytes, `cap` means the same thing, the
in-place reuse test in §5.3 reads the same field, and the `Alloc` cost model in
§7 is **defined** rather than measured, so not one number a program can see
moves when the free lists land. That makes the allocator replaceable under a
green test suite instead of something that has to be co-developed with two
backends.

What it costs until then, stated so it is not discovered: an allocation is a
`malloc` call rather than six inline instructions, which is the difference
between roughly twenty cycles and roughly five on the fast path. That is a real
number and it is the right one to pay first, because a size-class allocator that
is wrong is a heap corruption and a `malloc` that is slow is a profile.

**`cap` is the block's usable capacity, not the value's length**, and §5.3's
doubling is the reason it is now routinely larger. A `[T]`'s element count is in
its descriptor and a `Str`'s byte count is in its view, so neither reads `cap`
to know how long it is; `cap` is read by `buri_rt_free`, to recover the layout
the block was made with, and by §5.3's headroom test. Two consequences worth
naming before they are met. The heap accounting (`buri_rt_heap_stats`) counts
capacity, so `live_bytes` after a build loop is up to twice the bytes the values
hold — it is a measurement of `malloc`, and the `Alloc` charge in §7 is a
definition over the *types*, so nothing a program can observe moves. And the
release glue for a `[T]` block walks `cap / stride` elements, which is why
§5.3's fast paths are restricted to element types that hold no counted
references: spare capacity and a capacity-driven drop walk cannot both be right.

When the size-class allocator lands it will round a request up to its class, so
`cap` will exceed the request even without §5.3. That is the same property, and
`buri_rt_grown_capacity` should then round to a class rather than double, which
makes the doubling free — the block was going to be that big anyway.

The growth path, in full:

- **Small (≤ 32 KiB payload).** Size-class segregated free lists over 1 MiB
  chunks from `mmap`. Classes are 16, 32, 48, 64, 80, 96, 112, 128, then powers
  of two with two intermediate steps each, to 32 KiB. Allocation is: index the
  class from the size (a shift and a table lookup, both constant-folded when the
  size is a compile-time constant, which it is for every fixed-size aggregate),
  pop the free list, done — about six instructions, and both backends open-code
  it.
- **Large (> 32 KiB).** Straight to `mmap`, rounded to a page, `munmap` on free.
- **Chunks are never returned to the OS** in v1. A high-water-mark heap is the
  right default for a compiler-shaped workload and the wrong one for a
  long-running server; the growth path is a decommit pass on a chunk whose free
  list is entirely free, and it is a runtime change with no compiler involvement.

Non-atomic counts and a lock-free-because-single-threaded allocator are both
conditional on the language having no threads (§1). If threads are ever added,
this is the cost: reference operations become atomic, and the allocator grows
per-thread caches. That is a real cost — atomic RC is roughly 2-3× the
uncontended cost of non-atomic — and it should be priced into any future
concurrency proposal rather than discovered by it. Writing it here is how it gets
priced.

## 6. What this costs, honestly

Reference counting is not free, and the places it is not free in this language
are:

- **Shared, deeply nested, short-lived data.** Building a large JSON tree and
  dropping it walks the whole tree twice — once to build, once to free — where a
  generational collector would have dropped a nursery. `core/json`'s parser is
  the shape most exposed to this.
- **`Str` views keep their parent alive.** `s.splitOnce(",")` on a 10 MB string
  and keeping one 3-byte half retains all 10 MB. This is a real footgun and it
  is the price of `slice` being pure (`str.buri:3-4`). It is documentable rather
  than fixable — a copying `slice` would have to name `Alloc`, which is a
  language change — and `core/str` now says so where `slice` is declared.

  **Ruled on, and closed.** This was carried in `OPEN-QUESTIONS.md` as a
  language question because the two alternatives — copying above a ratio, or
  copying on a proven retention — change `slice`'s and `splitOnce`'s signatures
  or the middle end's obligations. The ruling is that it is **neither**: slicing
  keeps the parent, and *how* a view's storage is managed is an implementation
  detail of the runtime, not a property of the language. `slice` promises a
  view, `Alloc` is where allocation is named, and nothing in either promise
  mentions reference counts. So the strategy underneath — Perceus today, with or
  without the compaction pass in §5.3's known extensions — can change under a
  green suite without a SPEC amendment, exactly as §5.4's allocator can. What is
  *not* free to change is `slice` being pure, and that is why the language
  question was worth asking before the answer was written down.
- **A count on every heap value even where nothing shares.** Elision removes most
  of the traffic and none of the 16 bytes.

The alternative that would fix all three is a generational copying collector, and
§3 gives the reason it is not available. This is the trade, taken deliberately.

## 7. `Alloc`, natively: a defined cost model

`design/STANDARD-LIBRARY.md` §1 settles the important half already: **a
byte-exact cost model has to be *defined*, not measured**, or the numbers are
not reproducible across backends and every test that asserts one is flaky. That
stands, and it decides everything below.

### 7.1 The model

The charge for an allocating operation is a function of the **types**, computed by
`middle::layout`, so both backends and both platforms charge the same number for
the same program. It is not a measurement of what the allocator did.

| Operation | Charge, in bytes |
|---|---|
| A `[T]` of *n* elements | `16 + n * stride(T)` |
| A `Str` of *n* UTF-8 bytes | `16 + n` |
| A `Str` **view** (`slice`, `trim`, `splitOnce`) | `0` |
| A closure environment of fields *F* | `16 + size(record(F))` |
| A heap-promoted struct, tuple or enum payload | `0` |
| `allocate(ctx, n)` | `n` |

`stride(T)` and `size(...)` are VALUE-MODEL.md §5's layout, which is defined —
declaration order, natural alignment, no reordering — so the numbers are stable
under any change to the allocator and unstable only under a change to the layout,
which is a breaking change by construction and says so here.

Two rows deserve their reasons:

- **A view charges nothing** because the language says so: `slice`, `trim` and
  `splitOnce` are declared without an `Alloc` bound (`str.buri:26-45`). The
  accounting has to agree with the type system or the type system is lying.
- **A fixed-size construction charges nothing** even when the implementation
  heap-allocates, because SPEC 10.5 says "fixed-size construction — struct
  literals, tuples, enum payloads, array literals, closures, `Template`s — never
  requires `Alloc`". The model counts what the *language* says allocates. A model
  that counted implementation allocations would make `Alloc` accounting depend on
  escape analysis, and a number that moves when the optimizer improves is not a
  number a test can assert.

Making it a definition also makes it a **commitment**: a change to any row is a
breaking change to observable behaviour, exactly as
`design/STANDARD-LIBRARY.md` §1 says. It
goes in `core/effect`'s own source next to the `Alloc` declaration, where a reader of
the effect meets it — and it is there now, as a table above `effect Alloc`.
`middle::layout`'s `charge_list`, `charge_str`, `charge_closure_env`,
`charge_allocate` and `CHARGE_VIEW` are the same rows as code, and
`core/alloc`'s `strBytes`, `listBytes` and `closureBytes` are them again as
something a program can call. Three spellings of one definition is two too many
to change silently, which is exactly what "commitment" is supposed to mean.

### 7.2 The three allocator types

`GeneralPurpose`, `Arena` and `FixedBuffer`
(`cli/src/docs/guide/standard-library.md` "Allocators") are budgets and
accounting policies over the one real allocator. They are not three allocators.

- **`GeneralPurpose`** — unbounded, counts. `allocate` returns
  `Region(bytes_charged)` and adds to a running total the type exposes.

  **As built, the total is not *in* the type.** Buri has no mutation, so a
  running total cannot live in the struct that reports it; `GeneralPurpose` is
  a handle into a counter table in `memory.rs`, exactly as
  `core/testing/context`'s captured stdout is a handle. The type still
  *exposes* the total — `gp.stats()` — which is what this row meant; where it
  lives is the part that had to change. One consequence is worth stating
  because a program can see it: a copy of an allocator shares its counter,
  because the handle is the identity.
- **`FixedBuffer(n)`** — a budget of *n* bytes. Exceeding it **aborts**. This is
  forced, and it is the right answer: `allocate` returns `Region`, not
  `Result<Region, _>` (`effect.buri:19`), so there is no value to report failure
  with; and SPEC 10.5 already says `Alloc` "can fail (out of memory)", while SPEC
  6.10 says an abort is what a failure with no value to return does. So exceeding
  a `FixedBuffer` is `$abort("allocation budget exhausted")`, with the budget and
  the request in the message.
- **`Arena`** — in v1, `GeneralPurpose` with its own separate counter. It does
  *not* free in bulk, and pretending otherwise would be the "synthetic number
  rather than a measurement" the JavaScript backend was rightly criticised for
  (`design/STANDARD-LIBRARY.md` §2).

  What would make `Arena` real is a language construct that bounds a context's
  lifetime — a scoped context, such that everything allocated under it is
  unreachable at the end of the scope. That is a language proposal, not a backend
  feature, and it is worth naming precisely so nobody attempts the backend half
  first: without it, an arena in this language has no scope to end at (§4).

### 7.3 The hook is already there

`design/STANDARD-LIBRARY.md` §2 records the non-obvious part, and it survives
intact: every
allocating intrinsic is already handed the context and discards it —
`$list_map(xs, c, f)`, `$str_split(s, c, sep)`, `$list_range(c, a, b)`. Routing
it needs no change to any signature.

Natively there is one refinement. VALUE-MODEL.md §8 says a context of zero-sized
implementations is itself zero-sized and is dropped from every signature, and
`HostAlloc` is `struct HostAlloc {}` (`host.buri:18`) — zero-sized. So on the
default host context, the allocator argument is dropped and the intrinsics call
the global allocator directly, which is correct and free. A `FixedBuffer` or a
counting `GeneralPurpose` is *not* zero-sized — it holds a budget and a total —
so a context binding one is a real record and the intrinsics receive it. The
accounting costs exactly the programs that ask for accounting.

`design/STANDARD-LIBRARY.md` §2 also settles that no reserved context slot is
needed, and
notes "a native backend knows the layout statically and does neither". That is
right and it is now stronger than it sounds: natively there is no scan and no
cache because there is no context value to scan.

#### 7.3.1 Amendment: the hook is there and the *charge* is not

`core/alloc` shipped and this section's last paragraph did not survive contact.
The correction matters, because the paragraph reads as "the accounting is
nearly free" and it is not.

**A context argument is dropped from every `buri_rt_*` call, whatever it
weighs** (`cranelift/runtime.rs`'s `runtime_call`). That is not an oversight to
undo: it was forced by the *first* program to bind a non-zero-sized allocator —
`context { Alloc: alloc() }` from `core/testing/context` — which spread one
extra argument into a C call that has no parameter for it and put every
argument after it in the wrong register. So the intrinsics do **not** receive a
counting allocator; the runtime function that builds the list never learns
which allocator asked for it.

The JavaScript backend has the context and cannot use it either, for an
unrelated reason: the charge for a `[T]` is `16 + n * stride(T)`, and an
untyped runtime does not have `stride(T)`.

So what an allocator is told about is **`allocate(ctx, n)` and nothing else**,
identically on both backends. Every other row of §7.1 is still a charge — the
model is a definition, and a definition does not need a reporter to be true —
but nothing counts it. `core/alloc`'s module comment states that boundary where
a user meets it, and `cli/tests/conformance/lib/memory/` pins it on both
backends.

Closing the gap is a wave of its own, and the shape of it is now clear enough
to price: the charge has to be computed where the *type* is known, which is the
call site, and applied where the *length* is known, which is inside the runtime
function. That is either a middle-end pass that emits a charge beside each
allocating call (needing a length expression per intrinsic) or a widened ABI
that passes the charge and the counter handle into the runtime (needing every
`buri_rt_*` producer to take two more arguments). Both are two-backend changes,
and doing one backend alone breaks the one property the module has: that the
numbers agree.
