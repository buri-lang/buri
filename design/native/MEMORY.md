# Memory

The design notes state the problem and offer two answers: "the language has no
mutation and no destructors, so native either ships a GC or does escape
analysis with an arena per `Allocator` scope."

Both are wrong, and §3 and §4 say why. The answer is **non-atomic reference
counting with static elision and in-place reuse**, over a size-class
allocator, with `Allocator` as a *defined* accounting model rather than a
measurement.

## 1. What the language gives us

Four properties, and every decision below is downstream of them.

- **No mutation.** A value's fields are written once, at construction. There
  is no assignment, no interior mutability, no `&mut`.
- **No destructors.** Freeing is the implementation's business entirely;
  nothing in a program can observe when it happens or run code at that point.
- **No threads.** The language has no concurrency construct, `core/effect`
  grants no effect that produces one, and nothing in the standard library
  spawns anything.
- **Effect-carrying values cannot be captured.** SPEC 10.6, checked. So a
  closure's environment is plain data.

## 2. Immutability implies acyclicity, and that is the whole argument

> **Lemma.** In a Buri program, the points-to graph over heap values is acyclic.

A value's fields are set once, at construction, from expressions evaluated
before the construction. So every reference in a value points to a value that
already existed, and "already existed" is a strict partial order. A cycle
would need a value pointing at something constructed after it, which requires
either assignment after construction (there is none) or a recursive binding
whose right-hand side refers to the binding itself. That second one is the
hazard, and it cannot happen:

- **Recursive *types* are not recursive *values*.**
  `enum Rose { Node([Rose]) }` is fine; every `Rose` is built from `Rose`s
  that already exist. The recursion is in the type, and a type is not a heap
  object.
- **Recursive *functions* are not heap values.** Recursion goes through a
  top-level `fn`, which the middle end resolves to a code pointer
  (`monomorphize.rs`, `Callee::Func`). A code pointer is not reference
  counted.
- **A lambda cannot refer to itself.** `ExprKind::Lambda` captures a list of
  already-bound locals (`typed.rs`); there is no `let rec`, and a
  `let f = fn(x) => f(x)` fails name resolution because `f` is not in scope in
  its own initialiser.
- **A context cannot close a cycle.** SPEC 10.6 again: nothing effect-carrying
  is ever captured, so a context never ends up inside a value that the context
  also reaches.

That lemma is what makes reference counting **sound and complete** here: sound
because dropping to zero means unreachable, complete because there is no cycle
left over for a collector to find. Refcounting in a language with mutation is
a memory leak with extra steps; in this language it is a complete collector.

Two test suites defend the lemma from opposite ends.
`cli/tests/conformance/lib/memory/` runs on every backend and pins the cost
model §7.1 defines. The leak half lives in `cli/tests/native/stencil.rs`,
because no `test` block can assert it from inside the language —
`buri_rt_heap_stats` is not reachable from Buri and should not be.
`nothing_is_leaked` and `the_glue_balances` link a probe against the runtime,
run a program over the shapes that would break the lemma (a `Str` in a struct,
a `Str` in an enum payload, a closure environment carrying its own release
function, a `[Str]` whose elements are released by the block's own glue, a
boxed field), and assert **zero live blocks at exit** with a nonzero total. A
future language feature that introduces a cycle fails there rather than in
production.

There is no "runtime built with leak checking on" to arrange. `buri_rt_alloc`
and `buri_rt_free` always keep the four counters `buri_rt_heap_stats` reports,
because a relaxed add beside a `malloc` is not a cost anybody can measure, and
a diagnostic that only exists in a special build is a diagnostic nobody runs.
Marking a block immortal (`buri_rt_make_immortal`) removes it from the live
count, so a leak check does not report every string literal.

## 3. Why not a tracing GC

Direct conflict with CODEGEN-LLVM.md §0's second instruction.

A tracing collector has to find the roots, which means knowing which stack
slots and which registers hold pointers at every point a collection can
happen. There are two ways to have that in LLVM, and both are excluded:

- **Precise, via `gc.statepoint`.** Every GC-reachable pointer has to live in
  a stack slot the runtime can enumerate at a safepoint, which is `alloca` +
  reload around every call. That is exactly the `alloca` form CODEGEN-LLVM.md
  §0's second instruction says not to generate, and running `mem2reg` over it
  does not help: the whole point of a statepoint is that the value is *not* in
  a register across the call. The instruction and the collector are
  incompatible, and the instruction is the one with a reason behind it.
- **Conservative, scanning the stack for anything that looks like a pointer.**
  This language's most common heap value is `[Int]`, an array of arbitrary
  64-bit integers, any of which may be numerically equal to a live address.
  Conservative scanning turns "a program computed a large number" into "a
  program retains an arbitrary allocation", and the retention is
  data-dependent and irreproducible. In a toolchain whose central claim is
  that two builds of a tree agree byte for byte, an irreproducible heap is the
  wrong kind of nondeterminism to introduce.

A third cost applies to both: a collector has to be told about every pointer
the *runtime* holds too, so 203 runtime functions each grow a rooting
discipline.

## 4. Why not an arena per `Allocator` scope

The effect system does not carry the information it would need. **`Allocator` says
a function allocates. It does not say when the allocation dies.**

An arena needs a scope: a point at which everything allocated since some
earlier point becomes unreachable, all at once. Look for one:

- `Allocator` is a **bound on a context** (`effect.buri`), and a bound propagates.
  `list.map` is `<C: Allocator>`, so every caller of `map` is `Allocator`-bounded, and
  so is every caller of *those*. In any program that maps a list, `main` is
  `Allocator`-bounded and the "Allocator scope" is the program.
- `Region` is a **value** (`effect.buri`: `export struct Region(export I64)`),
  returned by `allocate` and freely storable in a struct, returnable from a
  function, and placeable in a list. It is not a scope and it does not nest.
- There is no `with`, no `using`, no scoped-context expression. A context is
  built by `context { ... }` and lives as long as anything referring to it.

So an arena hung on the `Allocator` bound is entered at `main` and left at exit,
which is the never-free strategy under a different name.

Escape analysis would help — a value that provably does not outlive its frame
can be stack-allocated — *as an optimization on top of* something correct
without it. It is not a strategy on its own, because the cases it cannot prove
need an answer, and "leak it" is not one.

### 4.1 And why not never-free

Viable for a compiler or a `grep`; not viable here. `core/http` is in the
standard library, `core/net` grants `fetch`, and a server that never frees is
not a server. `buri test --watch` (BUILD-AND-WATCH.md §4) is a long-lived
process too. Choosing a memory strategy that only works for programs that exit
quickly is choosing which programs the language is for, and nothing else in
this language has made that choice.

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

`incref` is branchless after the null test: a load, a saturating add (one
`add` plus one `cmov` on x86-64, one `adds`+`csinv` on aarch64), a store.
`IMMORTAL` stays `IMMORTAL` under saturation, which is what makes the immortal
case free on the increment side, where the traffic is.

`decref` has two branches, both well predicted. `IMMORTAL` is taken always or
never per call site in practice, and `rc == 1` is the uncommon case in shared
data and the common case in linear data — which is what §5.3 exploits.

Both are **open-coded by both backends**, never called. A call per reference
operation is the single reason reference counting has a reputation, and both
native backends emit these as a handful of instructions with no spills: two
stencils in the debug one (CODEGEN-STENCIL.md §6), inlined IR in LLVM. The
`drop_T` in the cold path *is* a call, to a generated per-type function.

Null checks are eliminated wherever the layout says the pointer is non-null,
which is everywhere except a niche-encoded `Option` (VALUE-MODEL.md §6). LLVM
gets this for free from `nonnull` (CODEGEN-LLVM.md §3).

#### The shared fork

The sequences above are the **unshared** arm. Since the multi-threaded mark
was reserved (VALUE-MODEL.md §2.1), each operation is two counts behind one
branch:

```
incref(p):                             decref(p):
  if p == null: return                   if p == null: return
  if cap[63]: atomic_incref(p); return   if cap[63]: atomic_decref(p); return
  ... the sequence above ...             ... the sequence above ...

atomic_incref(p):                      atomic_decref(p):
  d = load p[-16] != IMMORTAL            d = load p[-16] != IMMORTAL
  atomicrmw add p[-16], d relaxed        if atomicrmw sub p[-16], d acq_rel == 1:
                                           drop_T(p); free(p)
```

What the fork costs a program that takes the unshared arm is its own test, and
that number is worth stating: **two instructions** on each of the two
operations, on both instruction sets — `ldur x, [p, #-8]` + `tbnz x, #63` on
aarch64, `cmpq $0, -8(p)` + `js` on x86-64. The load reads the word beside the
count, in the same sixteen-byte header, so it is on a cache line the operation
was going to touch anyway. The branch is perfectly predicted, and the emitters
mark the unshared arm as the hot one, so it is the fallthrough and the atomic
arm is out of line. `design/PERFORMANCE.md` carries the measured cost.

#### Who sets the bit

**A program that can reach a task boundary marks every block it allocates. A
program that cannot marks none.** §5.5's asymmetry is the argument: an
over-set bit costs a copy, an under-set one is a silent aliasing bug.

Three pieces, each in the one place that can hold it:

- **`middle::rc::crosses_tasks`** asks the whole post-monomorphization program
  whether any intrinsic it can reach hands a value to another carrier — the
  `host.HostTasks` surface, by prefix, so a row track F adds is covered on the
  day it lands. The answer rides on `ir::Program::crosses_tasks`.
- **Both native backends** emit one call in `main` when it is true:
  `buri_rt_values_may_cross_tasks()`, immediately after `buri_rt_argv_init`
  and before anything allocates. The frame-threaded backend makes it too, even
  though it cannot fan out yet, because this is a fact about the *program*,
  where the other statement an artifact makes about itself
  (`buri_rt_frames_are_per_carrier`) is a fact about the *backend*.
- **`cli/runtime/memory.rs::finish`** ORs the mark into every `cap` it writes,
  out of one process-wide word. One relaxed load and one `or` per allocation,
  on a word written at most once in a program's life.

**Why the whole program and not the value.** A per-value mark has to be a
deep, type-directed walk of everything reachable from the call's arguments — a
`[Str]` handed to a step is a block whose *elements* the step counts, and a
`Str` inside a closure's environment is a block two carriers count — and a
*shallow* walk is exactly the under-set the asymmetry forbids. The
program-wide answer is sound by construction rather than by audit: a value
that reaches a carrier by a route the compiler cannot see — a block the
runtime built itself, a `Str` from `host.rs`, whatever an FFI hands in one day
— is marked anyway, because the *allocator* is what marks. What it costs is
atomic reference counting throughout a program that uses `core/tasks`, which
is the price §5.4 puts on threads. Narrowing it later is an optimisation over
an answer that is already correct.

The runtime's fan-out is gated on the same latch as well as on the frames one,
so an artifact that failed to make the call runs its tasks one after another —
slow, and never two carriers counting an unmarked block.

Two properties of the count survive the fork, and preserving them is why the
mark is a bit of `cap` and not of `rc`:

- **`IMMORTAL` saturation.** The atomic arms add and subtract a *delta* — `0`
  for an `IMMORTAL` block, `1` otherwise — which is the branchless `select` of
  the unshared arm written as the `atomicrmw`'s operand. A plain
  `fetch_add(1)` would wrap `u64::MAX` to zero and free every literal in the
  program.
- **The `rc == 1` uniqueness test** (§5.3) is not forked, and has a second
  half instead: **a marked block is never unique.** The count alone was right
  while exactly one carrier ran Buri code, on the premise that the caller
  holds the reference it is testing — and a *borrowed* parameter does not. A
  step of a `Tasks.parallel` reading `rc == 1` off its closure's list is one of
  several carriers reading the same `1`. So `buri_rt_unique_cap` answers
  `None` for a marked block whatever the count: the caller allocates and
  copies, and what an over-set mark costs is that copy.

`decref`'s atomic arm reads the count *before* the subtraction and frees on
`1`, rather than reading the count and then subtracting: two threads that each
read `1` from a separate load would each free the block. `acq_rel` is release
so that a thread's writes to the value reach whichever thread performs the
last decrement, and acquire so that thread sees them before it runs the drop
glue.

Both backends open-code the fork, and `cli/runtime`'s own `buri_rt_incref` and
`buri_rt_decref` take it too, so a block reached from a generic path is
counted the same way as one reached from emitted code.

### 5.2 Elision, which is where the cost goes

Naive reference counting increments on every parameter pass and decrements on
every scope exit, and it is slow. The fix is the one Koka's Perceus and Lean
4's runtime both use, and this language fits it better than either, because it
has no mutation at all. The paper is linked from
[../../reference/README.md](../../reference/README.md).

`middle::rc` computes, per parameter, whether the callee **owns** or
**borrows** it:

- A parameter is **borrowed** if the callee neither stores it in a constructed
  value, nor returns it, nor passes it to a function that owns it. A borrowed
  parameter needs no increment at the call and no decrement in the callee: the
  caller's own reference keeps it alive for the whole call.
- A parameter is **owned** otherwise, and the caller transfers a count.

The analysis is a fixpoint over the call graph, which is exact
(`monomorphize.rs`), so the answer is a fact rather than the conservative
approximation a language with dynamic dispatch would get. Every pure,
non-constructing operation in the standard library — `xs.fold(f, init)`,
`xs.any(pred)`, `s.startsWith(p)`, `s.indexOf(n)`, `xs.len()` — borrows
everything and touches no reference count at all.

On top of that, three local rules:

- **Drop the increment/decrement pair around a value that is dead immediately
  after.** The last use of a local transfers rather than copies.
- **`IMMORTAL` at compile time.** A literal, an interned constant aggregate
  (`generate.rs`'s `intern` moves to the middle end and applies to both
  backends), and any zero-sized value get no reference operations emitted at
  all, because the compiler knows they are immortal. The runtime `IMMORTAL`
  check is for values that reached a generic path.
- **Stack allocation for non-escaping aggregates.** The escape analysis §4
  declined to build a strategy on is a fine optimization: a struct constructed
  and consumed within one function, never stored and never returned, becomes
  an `alloca` (LLVM) or a frame range (the debug backend) with no header and
  no counts. This is the one place `alloca` is emitted, and it is emitted for
  a value that is never reloaded through a pointer, so CODEGEN-LLVM.md §0's
  second instruction is not violated — see CODEGEN-LLVM.md §2.3.

### 5.3 Reuse, which is where the copying goes

The other half of Perceus, and the reason an immutable language can be fast.

When a value is uniquely owned — `rc == 1` — nothing in the program can tell
the difference between building a new value and writing into the old one. The
runtime test guarding reuse is one compare against a header word the operation
was going to load anyway, and when it fails the fallback is allocate-and-copy,
which is what would have happened unconditionally without the feature.

#### What has landed, and where each fast path lives

A struct, a tuple, an enum and a closure record are **register or stack**
values — `MakeStruct` is a frame range in the debug backend and an LLVM
aggregate in the release one — and the only counted heap blocks are a `Str`'s
bytes, a `[T]`'s elements, a closure *environment*, and the box a recursive
field goes behind (VALUE-MODEL.md §5.2). So the two operations worth
optimizing are the two that build the first two, and both are done:

- **`[T]` append — `cli/runtime/list.rs`'s `append_dest`, behind `list.push`
  and `list.concat`.** Both are runtime calls on both backends
  (`stencil/runtime.rs`, `llvm/runtime.rs`), so the fast path lives in the
  runtime and is shared. Three paths: *in place* when the block is uniquely
  owned and `cap >= (len + n) * stride`, writing past the end and taking one
  more reference; *grown* when it is unique and out of capacity, allocating
  `max(needed * 2, 64)` so the next append is in place; *exact* otherwise. A
  loop of `n` pushes therefore allocates O(log n) times, which is where
  VALUE-MODEL.md §4.1's amortized O(1) comes from.
- **`Str` concatenation — `llvm/emit.rs`'s `concat` and
  `cli/runtime/text.rs`'s `buri_rt_str_concat`.** The same three paths, with
  the capacity test allowing for a view that starts inside its block:
  `(ptr - base) + alen + blen <= cap`. A template of *k* holes, or a fold that
  concatenates, is the shape this turns from O(k) allocations into O(log k).

  Two implementations rather than the list's one, because `str.concat` is
  **open-coded** where a backend can afford it. The release backend emits the
  three paths as instructions. The copy-and-patch backend cannot — a header
  load, two compares, three arms and a `memmove` are a dozen stencils and a
  block layout, against one `crt` stencil for a call — so it calls the
  runtime, which is what the `[T]` half does everywhere. This is a **promise
  about the count and not about where the code lives**: whichever backend
  compiled it, a chain of *n* appends onto a uniquely-owned string allocates
  O(log n) times, and `core/alloc`'s `count` and `total` say the same numbers
  in a debug build as in a release one (CODEGEN-STENCIL.md §5.0.1).

**Why the in-place write is unobservable.** `rc == 1` means exactly one live
value refers to the block. Every operation that produces a *new* view of a
block increfs its base before answering (`cli/runtime/text.rs`), so a second
view would be a second count; and static elision (§5.2) never duplicates a
reference without an `incref` — a borrowed parameter *aliases* the caller's
reference rather than adding one. So the aliases elision leaves behind are
copies of that one value, carrying the same `ptr` and the same `len`, and a
write that starts at `ptr + len` is invisible to all of them. The correctness
of §5.2's counting is what licenses §5.3's mutation, and there is no separate
argument to make.

Which means a *wrong* count here is not merely a leak or a use-after-free: it
is a licence to overwrite something live. Writing the fast paths turned up two
places where `middle::rc` got the count wrong, and they are worth naming
because they are the shape the next one will have. A local scrutinized by
**two** consuming `match`es was dropped by each of them, because the first one
erased it from the liveness the second computed. And a **borrowed local handed
to a construct beside a sibling holding its last mention** — `f(s, g(s))`, or
`"${s} … ${s.len()}"` — was dropped after the sibling, while the construct was
still holding uncounted words copied out of it. Neither was visible to the
balance checker, which counts operations rather than orders them; both were
visible the moment an allocation reused the freed block.

**Growth policy: doubling with a floor of 64 bytes**, applied only when the
left operand is uniquely owned. A shared operand is not the one being built,
so it gets an exact allocation and no speculative capacity. The floor is
`layout::GROWTH_FLOOR` for the backends and `BURI_RT_GROWTH_FLOOR` for the
runtime — two constants for two crates that never link against each other, and
a disagreement between them costs a reallocation rather than an answer.

*What* is doubled differs between the two payloads, deliberately. A `[T]`
append doubles the **old capacity** (`buri_rt_grown_capacity`); a `Str`
concatenation doubles the **result**, `max(n * 2, floor)`. Both are amortized
O(1). They are not unified because a `Str`'s growth is written in three
places, and the three have to allocate the same number of times.

#### What is excluded, and why

- **A counted element type — `[Str]`, `[(Str, Int)]` — takes neither the
  in-place path nor the over-allocation.** Two correctness reasons. Writing at
  index `len` would drop whatever reference that slot already held without a
  `decref`, because a slot past the end of one descriptor may hold an element
  a *longer*, now-dead descriptor put there. And the generated release glue
  for a `[T]` block walks **`cap / stride`** elements (`stencil/glue.rs`'s
  `Elems`, `llvm/emit.rs`'s `Job::ReleaseElems`), so spare capacity would have
  the drop walk slots nothing ever wrote. Lifting it means adding a
  per-element *release* glue beside the `retain` this ABI already passes, and
  making that walk follow the element count rather than the capacity — a
  change in both backends rather than in the runtime.
- **Aggregate-cell reuse — the `S { ..old, field: new }` that Perceus is
  famous for — has no cell to reuse.** `middle::rc` computes the pairing
  (`FuncPlan::reuse`, behind `Options::reuse`, on by default) and it is
  correct as an analysis, but the construction it would rewrite does not
  allocate:

  ```buri ignore why="illustrative"
  match xs {
    .Cons(h, t) => .Cons(f(h), t),
    .Nil => .Nil,
  }
  ```

  compiles to a frame range and a store, not to a heap cell and a `decref`.
  The native suite's `a_struct_update_loop_allocates_nothing_per_iteration` is
  the measurement: a thousand struct updates allocate exactly as many blocks
  as ten. Emitting the conditional form — an `rc == 1` branch, a write, and an
  allocate-and-`decref` on the other side — would be two backends' worth of
  new IR to save an allocation that is not happening. The pairing therefore
  stays analysis, and that test is what would notice a layout change promoting
  aggregates to the heap.
- **Cross-block reuse** — pairing a dying value with a construction in a
  different basic block — is a known extension and is not in v1. So is reuse
  across a function boundary.
- **Same-layout reuse across types** — writing a `Point` into a dying `Pair`'s
  cell because both are two words — is excluded for the same reason as the row
  above it, plus one of its own: it would make the reuse decision depend on
  the layout table agreeing about two unrelated types, and a layout change
  would then silently change which programs mutate.

**The sanctioned direction.** The performance story for this design is Roc's,
and Roc is the existence proof that it works for a pure language shipped to
users rather than only in a paper: Perceus-style reference counted, no tracing
collector, speed from **opportunistic in-place mutation at refcount 1**, with
as much of the ownership decided statically as the compiler can manage so the
runtime `rc == 1` test is skipped where the answer is already known. Koka
calls the paradigm this enables *functional but in-place*.

The order of work is: more static ownership (fewer `rc == 1` tests, not faster
ones), then cross-block reuse, then reuse across a function boundary. What is
explicitly **not** on the path is a tracing collector beside the counts (§3),
or atomic counts before the language has threads (§5.4).

- Reinking, Xie, de Moura and Leijen, *Perceus: Garbage Free Reference Counting
  with Reuse*, PLDI 2021 — the algorithm, and the FBIP framing.
- Roc's implementation, and Teeuwissen's *Reference Counting with Reuse in Roc*
  (Utrecht, 2023) — the same algorithm in a shipped compiler, including where
  the reuse analysis pays and where it does not.

### 5.4 The allocator underneath

Single-threaded, no locks, no atomics.

**v1 is `malloc`-backed and has no size classes.** `buri_rt_alloc(payload)` is
one allocation of `16 + payload` bytes at 16-byte alignment with the header
written, and `buri_rt_free` returns it — a call, not an open-coded sequence.
The rest of this section is the growth path, and the separation is deliberate:
everything *observable* about allocation is settled either way. The header is
the same 16 bytes, `cap` means the same thing, §5.3's in-place reuse test
reads the same field, and §7's cost model is **defined** rather than measured,
so not one number a program can see moves when the free lists land. That makes
the allocator replaceable under a green test suite.

What it costs until then: an allocation is a `malloc` call rather than six
inline instructions, roughly twenty cycles against roughly five on the fast
path. That is the right one to pay first, because a size-class allocator that
is wrong is a heap corruption and a `malloc` that is slow is a profile.

**`cap` is the block's usable capacity, not the value's length**, and §5.3's
doubling is why it is now routinely larger. A `[T]`'s element count is in its
descriptor and a `Str`'s byte count is in its view, so neither reads `cap` to
know how long it is. `cap` is read by `buri_rt_free`, to recover the layout
the block was made with, and by §5.3's headroom test. Two consequences follow.
The heap accounting (`buri_rt_heap_stats`) counts capacity, so `live_bytes`
after a build loop is up to twice the bytes
the values hold — it measures `malloc`, and §7's charge is a definition over
the *types*, so nothing a program can observe moves. And the release glue for
a `[T]` block walks `cap / stride` elements, which is why §5.3's fast paths
are restricted to element types that hold no counted references: spare
capacity and a capacity-driven drop walk cannot both be right.

When the size-class allocator lands it will round a request up to its class,
so `cap` will exceed the request even without §5.3, and
`buri_rt_grown_capacity` should then round to a class rather than double —
which makes the doubling free.

The growth path, in full:

- **Small (≤ 32 KiB payload).** Size-class segregated free lists over 1 MiB
  chunks from `mmap`. Classes are 16, 32, 48, 64, 80, 96, 112, 128, then
  powers of two with two intermediate steps each, to 32 KiB. Allocation is:
  index the class from the size (a shift and a table lookup, both
  constant-folded when the size is a compile-time constant, which it is for
  every fixed-size aggregate), pop the free list, done — about six
  instructions, and both backends open-code it.
- **Large (> 32 KiB).** Straight to `mmap`, rounded to a page, `munmap` on
  free.
- **Chunks are never returned to the OS** in v1. A high-water-mark heap is the
  right default for a compiler-shaped workload and the wrong one for a
  long-running server. The growth path is a decommit pass on a chunk whose
  free list is entirely free, and it is a runtime change with no compiler
  involvement.

Non-atomic counts and a lock-free-because-single-threaded allocator both
depend on the language having no threads (§1). If threads are ever added, the
cost is: reference operations become atomic — roughly 2-3× the uncontended
cost of non-atomic — and the allocator grows per-thread caches. Both halves
are now in the tree and neither has been paid. The first is §5.1's fork, two
instructions until something sets the bit. The second is this:

**The per-thread caches.** A free list per thread in front of `malloc`, keyed
on the **exact** payload size for payloads up to 256 bytes, with a byte budget
per thread that is one process-wide number divided by the carrier count. Three
decisions in that sentence:

- **Exact sizes, not size classes.** A class allocator rounds a request up, so
  `cap` comes back larger than the payload asked for, and the release glue of
  a `[T]` would then walk slots nothing wrote.
  `buri_rt_grown_capacity` may overshoot only because the fast paths using it
  are restricted to element types holding no references; a cache is under no
  such restriction, since every block in the program passes through it. Keying
  on the exact size gives a cache with *no* semantic footprint. When the
  size-class allocator of the growth path lands, it is the thing that decides
  `cap`, and this cache becomes its per-thread front end rather than a second
  answer to the same question.
- **256 bytes.** Where this language's allocation histogram is: a short
  `Str`'s bytes, a fixed-size aggregate, a list below the first few doublings
  of the growth floor. A block above it is rare enough that a `malloc` per
  block is the right answer.
- **A budget divided by the carriers, not multiplied by them.** The budget is
  stated for the process and split, so the cache's total footprint is a
  property of the program rather than of how wide the carrier pool is: sixteen
  carriers get a sixteenth each rather than sixteen times the memory.

The block's own header carries the free list's link — `rc` holds the next
block's pointer while it is dead — so the lists cost one head per size per
thread and not one byte per block.

**`buri_rt_heap_stats` is unmoved by any of it.** It counts blocks the
*program* asked for, not calls this file made to `malloc`: a cache hit still
increments `live_blocks` and `total_blocks`, and a free still decrements
`live_blocks` before the block goes into a list. So a cached block is not live
and is not a leak — it is memory this runtime holds, exactly as an allocator
holds a free list — and `cli/tests/native`'s allocation-count assertions keep
measuring the compiler's elision rather than this file's hit rate.

### 5.5 The same opportunity in JavaScript, without a count

JavaScript is garbage collected, so `rc` did not run for it at all, and
`$list_push` was `xs.slice()` and a `push` — O(n) per call, and O(n²) for the
loop that is the most ordinary thing a program does with a list.

The opportunity is §5.3's exactly: *when nothing else can see the list, write
into it.* What is missing is `rc == 1`, because a garbage collector is
precisely the machinery that does not maintain a count.

#### A sticky bit, not a count

Every Buri list allocated by `runtime.js` carries `$u`, and it takes exactly
two values in its life: `true` when it is made, `false` the first time a
second reference to it comes into existence. Nothing ever puts it back. The
fast path is `xs.$u === true`.

A count would be better information and is not available. Its two halves fail
for different reasons:

- **The increments are cheap and static.** Where a second reference comes into
  existence is a question about the *tree*, and `middle::rc` already answers
  it for the native branch: the own/borrow fixpoint over the exact call graph,
  plus last-use liveness, is what places every `incref`. Running that half for
  JavaScript costs nothing new.
- **The decrements are the problem.** A decrement has to fire at the *exact*
  moment a reference goes away, and the whole point of a garbage collector is
  that the program does not say when that is. A closure that captures a list
  keeps it as long as the closure lives; a list handed to a host function is
  somewhere this compiler cannot follow. Emitting correct decrements means
  reconstructing the liveness the collector exists to hide — and a decrement
  we get wrong does not leak, it frees, which here means *writes into a list
  somebody is still reading*.

So the asymmetry decides it. **An over-set bit costs one copy.** A shared list
that nobody actually shares any more is copied once; the copy is fresh, so it
carries `$u === true`, and every operation after that writes through. A loop
pays at most one copy per *sharing event* rather than one per iteration.
**An under-counted reference is a silent aliasing bug**: two names for a list,
one of them written through, and a wrong answer with no crash to find it by.

#### Absence means *not ours*

The bit proves uniqueness positively. `xs.$u === true` is the whole test, and
`$own` in `runtime.js` writes the property and nothing else does. Everything
this backend did not allocate — a host array, an array from an interop
boundary, anything a future FFI hands in — carries no `$u`, so it reads as
shared and is copied. A new way for a foreign array to arrive is therefore
safe on the day it lands, because the only way to become writable is to have
been allocated here.

The mirror of that rule is that marking must not write on a foreign object
either. A list this backend made is marked by clearing its own `$u`; anything
else goes into a `WeakSet`, so `$share` hands a host array back exactly as it
received it. The set also holds the mark for a **struct, tuple or enum**, none
of which carry a bit: nothing writes into an aggregate — a functional update
spells its fields out or copies the array — so the only thing an aggregate's
sharing decides is what a field read out of it inherits.

#### The projection rule

Perceus's drop specialisation with the answer deferred.
`state = State { ..state, items: state.items.push(x) }` is the
record-accumulator fold, and it has to stay in place or the exercise just
moves the quadratic from the list to the struct around it. The field
`state.items` is a second reference to a list the struct still holds —
*unless* this expression is the last use of `state`, in which case the struct
is about to have no readers and its field is not shared by it.

`middle::rc` knows which of those it is, because last use is what its liveness
already computes. What it cannot know is whether `state` *itself* was shared
further up. So the compiler emits the question rather than the answer:
`$fromShared(state, state[1])` marks the field only if the parent is marked. A
`state` the caller kept was marked at the call, so the field is marked, so the
push copies — once, into a fresh unmarked struct, after which the loop runs in
place.

#### What the ownership half had to be told

Three things change in `middle::rc` under `Options::sharing`, and each is a
place where the native convention says something a garbage collector makes
false:

1. **A growing list operation consumes its receiver.** Natively `list.push`
   borrows and `append_dest` tests the count; here there is no count, so the
   receiver must be owned and a caller that keeps the list duplicates it. That
   duplication is the mark.
2. **A lambda's body is scanned, and it owns only what it binds.**
   `middle::closures` does not run on this branch — an arrow function closing
   over its scope is what the engine wants — so there is no lifted function
   carrying a plan of its own. Its parameters are owned, and the runtime
   functions that call one mark every element they hand over, which is the
   convention that makes that true. Its **captures are not**. Liveness in the
   enclosing scope says whether that scope reads a capture again; the body
   runs once per call, so a capture always has a next reader. Scanning the
   body against the enclosing `owned` set read `xs` as dying at the closure
   that captured it, emitted no mark, and let `$list_slice` truncate `xs` in
   place on the first call —
   `mapCtx(fn(c, i) => xs.slice(c, 0, i).len())` answered `0, 0, 0` where the
   answer is `0, 1, 2`. `Scan::enter_lambda` narrows the set to the body's own
   `let` bindings and parameters, which is what leaves the `foldCtx`
   accumulator writing through. `cli/tests/conformance/lib/memory/test/captures.buri`
   is one case per in-place operation.
3. **The base of a functional update is not a duplication.** The projections
   the update reads out of the base keep it live across its own siblings,
   which the generic scan reads as a second reference. True of a count; false
   of a reference that is being taken over.

And one thing narrows: the classifier. The native question is "does this value
hold a counted allocation", which a `Str` and a function value both answer yes
to. The sharing question is "can this value reach a **list**", because a list
is the only thing anything writes into — a `Str` is an immutable JavaScript
string and a function value is a closure. `Syntactic::for_lists` is the same
walk with different leaves: a `Point { x: Int, y: Int }` and a
`Result<Int, Str>` carry no marks at all.

The two questions have opposite safe directions, which is why they are not one
function with a flag threaded through. A type the native walk cannot answer
gets no operations and leaks; a type this walk cannot answer gets marked,
because the failure on this side is an aliased list nobody copied. A recursive
type is the same story from the other end: the native walk says `Yes` at its
depth bound because a type that reaches itself is behind a pointer and
therefore counted, while "reaches a list" is a least fixed point, and an
expression tree that reaches only itself and an `Int` reaches no list at all.

#### What it costs

The bit is a named property on a JavaScript array, measured against the
alternative — a wrapper object `{ a, u }` with the list inside it — on both
engines the suite runs under. Element reads, which outnumber everything else,
are identical across the marked array, the bare array and the wrapper (0.54 /
0.55 / 0.54 ms per million on JavaScriptCore; 0.99 / 1.00 / 1.01 on V8), and
mixing marked and unmarked lists through one call site costs nothing
measurable either — an array's elements do not live in its property backing
store. Growing is a wash. The wrapper's only edge is in stamping itself, and
against that it would put a dereference on every list access in the compiler
and the runtime, and would need wrapping and unwrapping at every host boundary
— the boundary whose whole property is that a foreign array is recognisable by
carrying nothing.

Two million pushes cost the same whether they are two hundred runs of ten
thousand or twenty runs of a hundred thousand, which is what linear means and
what `tests/language/sharing.rs` asserts. The artifacts grow by about one per
cent, which is the two helpers and the branch in each of the six operations.

## 6. What this costs, honestly

Reference counting is not free, and the places it is not free in this language
are:

- **Shared, deeply nested, short-lived data.** Building a large JSON tree and
  dropping it walks the whole tree twice — once to build, once to free — where
  a generational collector would have dropped a nursery. `core/json`'s parser
  is the shape most exposed to this.
- **`Str` views keep their parent alive.** `s.splitOnce(",")` on a 10 MB
  string and keeping one 3-byte half retains all 10 MB. This is a real footgun
  and it is the price of `slice` being pure (`str.buri`). It is documentable
  rather than fixable — a copying `slice` would have to name `Allocator`, which is
  a language change — and `core/str` now says so where `slice` is declared.

  **Ruled on, and closed.** The two alternatives — copying above a ratio, or
  copying on a proven retention — change `slice`'s and `splitOnce`'s
  signatures or the middle end's obligations. The ruling is **neither**:
  slicing keeps the parent, and *how* a view's storage is managed is an
  implementation detail of the runtime. `slice` promises a view, `Allocator` is
  where allocation is named, and neither promise mentions reference counts, so
  the strategy underneath can change under a green suite without a SPEC
  amendment, exactly as §5.4's allocator can. What is *not* free to change is
  `slice` being pure.

- **A count on every heap value even where nothing shares.** Elision removes
  most of the traffic and none of the 16 bytes.

A generational copying collector would fix all three, and §3 gives the reason
it is not available. This is the trade, taken deliberately.

## 7. `Allocator`, natively: a defined cost model

**A byte-exact cost model has to be *defined*, not measured**, or the numbers
are not reproducible across backends and every test that asserts one is
flaky. That decides everything below.

It is also what made the allocator types real. `GeneralPurpose`, `Arena` and
`FixedBuffer` were deferred while the only backend had a garbage collector, on
the grounds that a count would be synthetic. **What made them real was not the
native backend but this model**: the charge is computed from the types rather
than measured, so the same program charges the same number on both backends by
construction.

### 7.1 The model

The charge for an allocating operation is a function of the **types**,
computed by `middle::layout`, so both backends and both platforms charge the
same number for the same program. It is not a measurement of what the
allocator did.

| Operation | Charge, in bytes |
|---|---|
| A `[T]` of *n* elements | `16 + n * stride(T)` |
| A `Str` of *n* UTF-8 bytes | `16 + n` |
| A `Str` **view** (`slice`, `trim`, `splitOnce`) | `0` |
| A closure environment of fields *F* | `16 + size(record(F))` |
| A heap-promoted struct, tuple or enum payload | `0` |
| `allocate(ctx, n)` | `n` |

`stride(T)` and `size(...)` are VALUE-MODEL.md §5's layout, which is defined —
declaration order, natural alignment, no reordering — so the numbers are
stable under any change to the allocator and unstable only under a change to
the layout, which is a breaking change by construction and says so here.

Two rows deserve their reasons:

- **A view charges nothing** because the language says so: `slice`, `trim` and
  `splitOnce` are declared without an `Allocator` bound (`str.buri`). The
  accounting has to agree with the type system or the type system is lying.
- **A fixed-size construction charges nothing** even when the implementation
  heap-allocates, because SPEC 10.5 says "fixed-size construction — struct
  literals, tuples, enum payloads, array literals, closures, `Template`s —
  never requires `Allocator`". The model counts what the *language* says
  allocates. A model that counted implementation allocations would make
  `Allocator` accounting depend on escape analysis, and a number that moves when
  the optimizer improves is not a number a test can assert.

Making it a definition also makes it a **commitment**: a change to any row is
a breaking change to observable behaviour. The table sits above
`effect Allocator` in `core/effect`'s own source. `middle::layout`'s
`charge_list`, `charge_str`, `charge_closure_env`, `charge_allocate` and
`CHARGE_VIEW` are the same rows as code, and `core/alloc`'s `strBytes`,
`listBytes` and `closureBytes` are them again as something a program can call.
Three spellings of one definition is two too many to change silently.

### 7.2 The three allocator types

`GeneralPurpose`, `Arena` and `FixedBuffer`
(`cli/src/docs/reference/standard-library.md` "Allocators") are budgets and
accounting policies over the one real allocator. They are not three
allocators.

- **`GeneralPurpose`** — unbounded, counts. `allocate` returns
  `Region(bytes_charged)` and adds to a running total the type exposes.

  **As built, the total is not *in* the type.** Buri has no mutation, so a
  running total cannot live in the struct that reports it. `GeneralPurpose` is
  a handle into a counter table in `memory.rs`, exactly as
  `core/host/testing`'s captured stdout is a handle. The type still exposes
  the total through `gp.stats()`. One consequence a program can see: a copy of
  an allocator shares its counter, because the handle is the identity.
- **`FixedBuffer(n)`** — a budget of *n* bytes. Exceeding it **aborts**. That
  is forced, and it is the right answer: `allocate` returns `Region`, not
  `Result<Region, _>` (`effect.buri`), so there is no value to report failure
  with; SPEC 10.5 already says `Allocator` "can fail (out of memory)"; and SPEC
  6.9 says an abort is what a failure with no value to return does. So
  exceeding a `FixedBuffer` is `$abort("allocation budget exhausted")`, with
  the budget and the request in the message.
- **`Arena`** — in v1, `GeneralPurpose` with its own separate counter. It does
  *not* free in bulk.

  What would make `Arena` real is a language construct that bounds a context's
  lifetime — a scoped context, such that everything allocated under it is
  unreachable at the end of the scope. That is a language proposal, not a
  backend feature: without it, an arena in this language has no scope to end
  at (§4). Until a scope exists, what an `Arena` is *for* is attribution — an
  arena per phase, answering "how much did parsing charge?" without
  subtracting two totals.

#### 7.2.1 Amendment: the scope exists, and holds the values too

The scoped context above is `core/alloc`'s **`scoped(ctx, body)`**, and the
value it hands the body is **`Scoped<C>`** — an attenuating wrapper on
`ReadOnly<C>`'s pattern (SPEC 10.8) whose type parameter carries no bound, so
it is not effect-carrying by mention and is expressible at all. Every effect
forwards to the wrapped `C`, one hand-written `impl<C: E> E for Scoped<C>` per
effect, because this language has no blanket implementations and no
delegation. `Allocator` is the one that does not.

**The arena is a real bump allocator over its own `mmap`s.** `arenaCreate`
maps nothing. A charge reserves its bytes from a 64 KiB block, mapping another
when that one is full, and a right-sized one of its own when the charge is
bigger than a block. `arenaRelease` `munmap`s every block when `body` returns.
`buri_rt_heap_stats` grew `arena_bytes` and `arena_released_bytes` so that "it
reserved pages **and** gave them back" is one assertion rather than two
half-ones.

**The values are in it too.** The native ABI drops the context argument from
every runtime call (§7.3.1), so the operation that builds a list cannot be
**told** which allocator asked for it. The answer is not to tell it. `scoped`
calls `buri_rt_alloc_arena_enter` before `body` and `buri_rt_alloc_arena_leave`
after, and for that dynamic extent — on that carrier — `buri_rt_alloc` serves
out of the arena and stamps `CAP_ARENA` (bit 62 of `cap`) into the header.
`buri_rt_free` reads that bit, does the accounting and returns; the pages go
back in one `munmap`.

That is an *over*-approximation of "charged to the `Scoped`": every allocation
in the extent is the scope's, whoever asked. It is the safe end of §5.5's
asymmetry — a block that should have been on the heap and is in the arena
leaves with the answer or dies with the scope, and occasionally costs a copy.
The active arena is a **thread-local**, and `rt.rs`'s carrier loop saves and
restores it around a stack switch, so it belongs to the *task* rather than to
the thread the task is on this turn — which is what makes a scope per request
safe, and what makes a task started inside a scope allocate on the platform
heap.

**What makes the bulk free sound is the copy at the boundary.** Exactly one
value leaves a scope — `body`'s answer — and `core/alloc::copyOut` deep-copies
it onto the caller's allocator before the pages go back. The copy is
generated, not called: `Helper::Copy` in the frame-threaded backend and
`Job::Copy` under LLVM are `Helper::Walk`'s recursion with
`buri_rt_copy_block` where the release walk has `decref`. The two functions
the walk reaches a block through are the whole of the runtime's half —
`buri_rt_copy_block`, and `buri_rt_copy_str` because a `Str`'s `ptr` points
*into* its block and has to be rebased. **A copy is not a share**: nothing in
the path increments a count, so the answer's blocks are fresh and uniquely
owned and the source's counts do not move.

The **invariant** the arrangement rests on is one sentence: *a value's
lifetime never exceeds the dynamic extent it was created in, except by being
the answer — and the answer is copied.* Buri has no mutable global state and
no way to stash a value where a scope cannot see it, and the runtime's own
tables (`testing.rs`, `net.rs`) keep Rust copies rather than Buri blocks. The
alternative — keep every arena alive for ever in case something escaped — is
rejected: an arena that is never released is not an arena.

A **closure** costs one word for this. `Ty::Fn` does not record what was
captured, so the environment block has always carried its own release function
in the word before the record; it now carries its copy function in the word
after that (`ENV_FIELDS` is 16). The alternative — one word pointing at a
static pair per type — costs the same eight bytes per type instead of per
closure, and puts a second load in front of every drop of every closure in the
language.

### 7.3 The hook is already there

The non-obvious part survives intact: every allocating intrinsic is already
handed the context and discards it — `$list_map(xs, c, f)`,
`$str_split(s, c, sep)`, `$list_range(c, a, b)`. Routing it needs no change to
any signature.

Natively there is one refinement. VALUE-MODEL.md §8 says a context of
zero-sized implementations is itself zero-sized and is dropped from every
signature, and `HostAllocator` is `struct HostAllocator {}` (`host.buri`) —
zero-sized. So on the default host context the allocator argument is dropped
and the intrinsics call the global allocator directly, which is correct and
free. A `FixedBuffer` or a counting `GeneralPurpose` is *not* zero-sized — it
holds a budget and a total — so a context binding one is a real record and the
intrinsics receive it. The accounting costs exactly the programs that ask for
accounting.

No reserved context slot is needed either: the JavaScript backend reads the
context's own binding and a native backend knows the layout statically.

#### 7.3.1 Amendment: the hook is there and the *charge* is not

The paragraph above reads as "the accounting is nearly free", and it is not.

**A context argument is dropped from every `buri_rt_*` call, whatever it
weighs** (`stencil/runtime.rs`, `llvm/runtime.rs`). That is not an oversight
to undo. The *first* program to bind a non-zero-sized allocator forced it —
`context { Allocator: alloc() }` from the test platform — which spread one extra
argument into a C call that has no parameter for it and put every argument
after it in the wrong register. So the intrinsics do **not** receive a
counting allocator, and the runtime function that builds the list never learns
which allocator asked for it.

The JavaScript backend has the context and cannot use it either, for an
unrelated reason: the charge for a `[T]` is `16 + n * stride(T)`, and an
untyped runtime does not have `stride(T)`.

So what an allocator is told about is **`allocate(ctx, n)` and nothing else**,
identically on both backends. Every other row of §7.1 is still a charge — a
definition does not need a reporter to be true — but nothing counts it.
`core/alloc`'s module comment states that boundary where a user meets it, and
`cli/tests/conformance/lib/memory/` pins it on both backends.

Closing the gap is a wave of its own. The charge has to be computed where the
*type* is known, which is the call site, and applied where the *length* is
known, which is inside the runtime function. That is either a middle-end pass
that emits a charge beside each allocating call (needing a length expression
per intrinsic) or a widened ABI that passes the charge and the counter handle
into the runtime (needing every `buri_rt_*` producer to take two more
arguments). Both are two-backend changes, and doing one backend alone breaks
the one property the module has: that the numbers agree.
