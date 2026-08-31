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
  already-bound locals (`typed.rs`); there is no `let rec`, and a `let f = fn(x) => f(x)`
  fails name resolution because `f` is not in scope in its own initialiser.
- **A context cannot close a cycle.** SPEC 10.6 again: nothing effect-carrying
  is ever captured, so a context never ends up inside a value that the context
  also reaches.

That lemma is what makes reference counting **sound and complete** here: sound
because dropping to zero means unreachable, complete because there is no cycle
left over that a collector would have to find. Refcounting in a language with
mutation is a memory leak with extra steps; in this language it is a complete
collector.

It is worth defending the lemma with tests rather than only with an argument,
and two things do it from opposite ends. `cli/tests/conformance/lib/memory/`
runs on every backend and pins the cost model §7.1 defines, so the charge a
program is told about is the same integer natively and on JavaScript. The leak
half — which no `test` block can assert from inside the language, because
`buri_rt_heap_stats` is not reachable from Buri and should not be — is in
`cli/tests/native/stencil.rs`: `nothing_is_leaked` and `the_glue_balances` link
a probe against the runtime, run a program over the shapes that would break the
lemma (a `Str` in a struct, a `Str` in an enum payload, a closure environment
carrying its own release function, a `[Str]` whose elements are released by the
block's own glue, a boxed field) and assert **zero live blocks at exit** with a
nonzero total. A future language feature that introduces a cycle fails there
rather than in production.

There is no "runtime built with leak checking on" to arrange: `buri_rt_alloc`
and `buri_rt_free` keep the four counters `buri_rt_heap_stats` reports, always,
because a relaxed add beside a `malloc` is not a cost anybody can measure and a
diagnostic that only exists in a special build is a diagnostic nobody runs. An
immortal block is removed from the live count when it is marked
(`buri_rt_make_immortal`), so a leak check does not report every string literal.
`cli/tests/native/runtime.rs` asserts the property on a corpus of one, from C;
`cli/tests/native/stencil.rs`'s leak tests are the same assertion over compiled
Buri programs.

## 3. Why not a tracing GC

Direct conflict with CODEGEN-LLVM.md §0's second instruction.

A tracing collector has to find the roots, which means knowing which stack slots
and which registers hold pointers at every point a collection can happen. There
are two ways to have that in LLVM, and both are excluded:

- **Precise, via `gc.statepoint`.** Every GC-reachable pointer has to live in a
  stack slot the runtime can enumerate at a safepoint, which is `alloca` +
  reload around every call. That is exactly the `alloca` form
  CODEGEN-LLVM.md §0's second instruction says not to generate, and running
  `mem2reg` over it
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

- `Alloc` is a **bound on a context** (`effect.buri`), and a bound propagates.
  `list.map` is `<C: Alloc>`, so every caller of `map` is `Alloc`-bounded, and so
  is every caller of *those*. In any program that maps a list, `main` is
  `Alloc`-bounded and the "Alloc scope" is the program.
- `Region` is a **value** (`effect.buri`: `export struct Region(export I64)`),
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
native backends emit these as a handful of instructions with no spills — two
stencils in the debug one (CODEGEN-STENCIL.md §6), inlined IR in LLVM. The
`drop_T` in the cold path *is* a call, to a generated per-type function.

Null checks: eliminated wherever the layout says the pointer is non-null, which is
everywhere except a niche-encoded `Option` (VALUE-MODEL.md §6). LLVM gets this for
free from `nonnull` (CODEGEN-LLVM.md §3).

#### The shared fork

The sequences above are the **unshared** arm. Since the multi-threaded mark was
reserved (VALUE-MODEL.md §2.1) each operation is two counts behind one branch:

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
that is the number worth stating: **two instructions** on each of the two
operations, on both instruction sets — `ldur x, [p, #-8]` + `tbnz x, #63` on
aarch64, `cmpq $0, -8(p)` + `js` on x86-64. The load is of the word beside the
count, in the same sixteen-byte header, so it is on a cache line the operation
was going to touch anyway; the branch is perfectly predicted; and the emitters
mark the unshared arm as the hot one, so it is the fallthrough and the atomic
arm is out of line. The measured cost is in `design/PERFORMANCE.md`.

#### Who sets the bit

**A program that can reach a task boundary marks every block it allocates. A
program that cannot marks none.** That is the whole policy, and the asymmetry
in §5.5 is the argument for it: an over-set bit costs a copy, an under-set one
is a silent aliasing bug, and there is no trade to make between an optimisation
that occasionally does not fire and one that is occasionally wrong.

Three pieces, and each is in the one place that can hold it:

- **`middle::rc::crosses_tasks`** asks the whole post-monomorphization program
  whether any intrinsic it can reach hands a value to another carrier — the
  `host.HostTasks` surface, by prefix, so a row track F adds is covered on the
  day it lands. The answer rides on `ir::Program::crosses_tasks`.
- **Both native backends** emit one call in `main` when it is true:
  `buri_rt_values_may_cross_tasks()`, immediately after `buri_rt_argv_init` and
  before anything allocates. The frame-threaded backend makes it too, even
  though it cannot fan out yet, because this is a fact about the *program* and
  the other statement an artifact makes about itself
  (`buri_rt_frames_are_per_carrier`) is a fact about the *backend*.
- **`cli/runtime/memory.rs::finish`** ORs the mark into every `cap` it writes,
  out of one process-wide word. One relaxed load and one `or` per allocation,
  on a word written at most once in a program's life.

**Why the whole program and not the value.** `middle::rc::sharing` computes
where a second *reference* comes into existence, which reads like the same
question and is not: that is a question about sites, and the mark is a question
about the transitive closure of a heap. A `[Str]` handed to a step is a block
whose *elements* the step counts; a `Str` inside a closure's environment is a
block two carriers count. So a per-value mark has to be a deep, type-directed
walk of everything reachable from the call's arguments — `Helper::Walk`'s
shape, which G5 generalises — and a *shallow* one is exactly the under-set the
asymmetry forbids. The program-wide answer is sound by construction rather than
by audit: a value that reaches a carrier by a route the compiler cannot see
— a block the runtime built itself, a `Str` from `host.rs`, whatever an FFI
hands in one day — is marked anyway, because the *allocator* is what marks.
What it costs is atomic reference counting throughout a program that uses
`core/tasks`, which is the price §5.4 puts on threads rather than a price this
shape adds. Narrowing it later is an optimisation over an answer that is
already correct.

**Silence is the safe answer, at both ends.** The runtime's fan-out is gated on
the same latch as well as on the frames one, so an artifact that failed to make
the call runs its tasks one after another — slow, and never two carriers
counting an unmarked block.

Two properties of the count survive the fork, and preserving them is why the
mark is a bit of `cap` and not of `rc`:

- **`IMMORTAL` saturation.** The atomic arms add and subtract a *delta* — `0`
  for an `IMMORTAL` block, `1` otherwise — which is the branchless `select` of
  the unshared arm written as the `atomicrmw`'s operand. A plain `fetch_add(1)`
  would wrap `u64::MAX` to zero and free every literal in the program.
- **The `rc == 1` uniqueness test** (§5.3) is not forked, and has a second
  half instead: **a marked block is never unique.** The count alone was right
  while exactly one carrier ran Buri code, on the argument that a thread
  holding no reference cannot make a second one — which has a premise, that the
  caller holds the reference it is testing, and a *borrowed* parameter does
  not. A step of a `Tasks.parallel` reading `rc == 1` off its closure's list is
  one of several carriers reading the same `1`. So `buri_rt_unique_cap` answers
  `None` for a marked block whatever the count: the caller allocates and
  copies, and what an over-set mark costs is that copy.

`decref`'s atomic arm reads the count *before* the subtraction and frees on `1`,
rather than reading the count and then subtracting: two threads that each read
`1` from a separate load would each free the block. `acq_rel` is release so a
thread's writes to the value reach whichever thread performs the last decrement,
and acquire so that thread sees them before it runs the drop glue.

Both backends open-code the fork, and `cli/runtime`'s own `buri_rt_incref` and
`buri_rt_decref` take it too, so a block reached from a generic path is counted
the same way as one reached from emitted code.

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
(`monomorphize.rs`) — so, unlike in a language with dynamic dispatch, the
answer is a fact and not a conservative approximation. `xs.fold(f, init)`,
`xs.any(pred)`, `s.startsWith(p)`, `s.indexOf(n)`, `xs.len()`: every pure,
non-constructing operation in the standard library borrows everything, and
therefore touches no reference count at all.

On top of that, three local rules:

- **Drop the increment/decrement pair around a value that is dead immediately
  after.** The last use of a local transfers rather than copies.
- **`IMMORTAL` at compile time.** A literal, an interned constant aggregate
  (`generate.rs`'s `intern` moves to the middle end and applies to both
  backends), and any zero-sized value get no reference operations emitted at all,
  because the compiler knows they are immortal — the runtime `IMMORTAL` check
  is for values that reached a generic path.
- **Stack allocation for non-escaping aggregates.** The escape analysis §4
  declined to build a strategy on is a fine optimization: a struct constructed and
  consumed within one function, never stored and never returned, becomes an
  `alloca` (LLVM) / a frame range (the debug backend) with no header and no
  counts. This is
  the one place `alloca` is emitted, and it is emitted for a value that is never
  reloaded through a pointer, so CODEGEN-LLVM.md §0's second instruction is not
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
and a closure record are **register or stack** values — `MakeStruct` is a frame
range in the debug backend and an LLVM aggregate in the release one — and the
only counted heap blocks
are a `Str`'s bytes, a `[T]`'s elements, a closure *environment*, and the box a
recursive field goes behind (VALUE-MODEL.md §5.2). So the two operations worth
optimizing are the two that build the first two, and both are done:

- **`[T]` append — `cli/runtime/list.rs`'s `append_dest`, behind `list.push`
  and `list.concat`.** Both are runtime calls on both backends
  (`stencil/runtime.rs`, `llvm/runtime.rs`), so the fast path is in the
  runtime and is shared. Three paths: *in place* when the block is uniquely
  owned and `cap >= (len + n) * stride`, writing past the end and taking one
  more reference; *grown* when it is unique and out of capacity, allocating
  `max(needed * 2, 64)` so the next append is in place; *exact* otherwise. A
  loop of `n` pushes therefore allocates O(log n) times, which is where
  VALUE-MODEL.md §4.1's amortized O(1) comes from.
- **`Str` concatenation — `llvm/emit.rs`'s `concat` and `cli/runtime/text.rs`'s
  `buri_rt_str_concat`.** The same three paths, with the capacity test allowing
  for a view that starts inside its block: `(ptr - base) + alen + blen <= cap`.
  A template of *k* holes, or a fold that concatenates, is the shape this turns
  from O(k) allocations into O(log k).

  Two implementations rather than the list's one, because `str.concat` is
  **open-coded** where a backend can afford it. The release backend emits the
  three paths as instructions; the copy-and-patch backend cannot — a header
  load, two compares, three arms and a `memmove` are a dozen stencils and a
  block layout against one `crt` stencil for a call — so it calls the runtime,
  which is what the `[T]` half does everywhere. This is a **promise about the
  count and not about where the code lives**: whichever backend compiled it, a
  chain of *n* appends onto a uniquely-owned string allocates O(log n) times,
  and `core/alloc`'s `count` and `total` say the same numbers in a debug build
  as in a release one. That the copy-and-patch backend once always allocated
  was a divergence in an observable, and it is fixed rather than documented
  (CODEGEN-STENCIL.md §5.0.1).

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

*What* is doubled differs between the two payloads, and deliberately. A `[T]`
append doubles the **old capacity** (`buri_rt_grown_capacity`); a `Str`
concatenation doubles the **result**, `max(n * 2, floor)`. Both are amortized
O(1) and the reason they are not unified is the paragraph above: a `Str`'s
growth is written in three places, and the three have to allocate the same
number of times or `core/alloc` reports a different total for the same program
depending on which backend compiled it.

#### What is excluded, and why

- **A counted element type — `[Str]`, `[(Str, Int)]` — takes neither the
  in-place path nor the over-allocation.** Two reasons, and both are
  correctness rather than caution. Writing at index `len` would drop whatever
  reference that slot already held without a `decref`, because a slot past the
  end of one descriptor may hold an element a *longer*, now-dead descriptor put
  there. And the generated release glue for a `[T]` block walks **`cap /
  stride`** elements (`stencil/glue.rs`'s `Elems`, `llvm/emit.rs`'s
  `Job::ReleaseElems`), so spare capacity would have the drop
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

  compiles to a frame range and a store, not to a heap cell and a `decref`.
  The native suite's `a_struct_update_loop_allocates_nothing_per_iteration` is
  the measurement rather than the claim: a thousand struct updates allocate exactly as many
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

**Both halves of that sentence are now in the tree, and neither has been paid
yet.** The first is §5.1's fork, which is two instructions until something sets
the bit. The second is this:

**The per-thread caches.** A free list per thread in front of `malloc`, keyed on
the **exact** payload size for payloads up to 256 bytes, with a byte budget per
thread that is one process-wide number divided by the carrier count. Three
decisions in that sentence:

- **Exact sizes, not size classes.** A class allocator rounds a request up, so
  `cap` comes back larger than the payload asked for — which the paragraph above
  anticipates and §5.3 forbids for one case: the release glue of a `[T]` walks
  `cap / stride` elements, so spare capacity in a block of counted elements is a
  walk over slots nothing wrote. `buri_rt_grown_capacity` is allowed to overshoot
  only because the fast paths that use it are restricted to element types holding
  no references, and a cache is under no such restriction — every block in the
  program passes through it. Keying on the exact size gives a cache with *no*
  semantic footprint: `cap` is what it always was, `layout_for` recovers the
  layout the block was made with, and the drop walk counts what it always
  counted. When the size-class allocator of the growth path lands, it is the
  thing that decides `cap`, and this cache becomes its per-thread front end
  rather than a second answer to the same question.
- **256 bytes.** Where this language's allocation histogram is: a short `Str`'s
  bytes, a fixed-size aggregate, a list below the first few doublings of the
  growth floor. A block above it is rare enough that a `malloc` per block is the
  right answer.
- **A budget divided by the carriers, not multiplied by them.** The budget is
  stated for the process and split, so the cache's total footprint is a property
  of the program rather than of how wide the carrier pool is: sixteen carriers
  get a sixteenth each rather than sixteen times the memory. That is the
  "sized for carrier count" this section asked for.

The block's own header carries the free list's link — `rc` holds the next
block's pointer while it is dead — so the lists cost one head per size per
thread and not one byte per block.

**`buri_rt_heap_stats` is unmoved by any of it.** It counts blocks the *program*
asked for, not calls this file made to `malloc`: a cache hit still increments
`live_blocks` and `total_blocks`, and a free still decrements `live_blocks`
before the block goes into a list. So a cached block is not live and is not a
leak — it is memory this runtime holds, exactly as an allocator holds a free
list — and `cli/tests/native`'s allocation-count assertions keep measuring the
compiler's elision rather than this file's hit rate.

### 5.5 The same opportunity in JavaScript, without a count

Everything above is the native branch. JavaScript is garbage collected, so `rc`
did not run for it at all, and `$list_push` was `xs.slice()` and a `push` — O(n)
per call, and O(n²) for the loop that is the most ordinary thing a program does
with a list. The same functional update that
costs a bump pointer natively cost a full copy per iteration on the backend that
defines the language.

The opportunity is §5.3's, exactly: *when nothing else can see the list, write
into it.* What is missing is the thing §5.3 asks — `rc == 1` — because a garbage
collector is precisely the machinery that does not maintain a count. So the
question is what to put in its place.

#### A sticky bit, not a count

Every Buri list allocated by `runtime.js` carries `$u`, and it takes exactly two
values in its life: `true` when it is made, `false` the first time a second
reference to it comes into existence. Nothing ever puts it back. The fast path
is `xs.$u === true`.

A count would be better information and is not available. The two halves of a
count fail for different reasons, and it is worth separating them:

- **The increments are cheap and static.** Where a second reference comes into
  existence is a question about the *tree*, and `middle::rc` already answers it
  for the native branch — the own/borrow fixpoint over the exact call graph,
  plus last-use liveness, is what places every `incref`. Running that half for
  JavaScript costs nothing new.
- **The decrements are the problem.** A decrement has to fire at the *exact*
  moment a reference goes away, and the whole point of a garbage collector is
  that the program does not say when that is. A closure that captures a list
  keeps it as long as the closure lives, and the closure's life is the engine's
  business. A list handed to a host function is somewhere this compiler cannot
  follow. To emit correct decrements we would have to reconstruct the liveness
  the collector exists to hide — and a decrement we get wrong does not leak, it
  frees, which here means *writes into a list somebody is still reading*.

So the asymmetry decides it. **An over-set bit costs one copy.** A shared list
that nobody actually shares any more is copied once; the copy is fresh, so it
carries `$u === true`, and every operation after that writes through. A loop
therefore pays at most one copy per *sharing event* rather than one per
iteration, which is the same asymptotics as the count with a worse constant on
a case that is rare. **An under-counted reference is a silent aliasing bug** —
two names for a list, one of them written through, and a wrong answer with no
crash to find it by. Between an optimisation that occasionally does not fire and
one that is occasionally wrong, there is no trade to make.

#### Absence means *not ours*

The bit proves uniqueness positively. `xs.$u === true` is the whole test, and
the property is written by `$own` in `runtime.js` and nowhere else. Everything
this backend did not allocate — a host array, an array from an interop
boundary, anything a future FFI hands in — carries no `$u`, so it reads as
shared and is copied. That is the safe direction by construction rather than by
audit: a new way for a foreign array to arrive is safe on the day it lands,
because the only way to become writable is to have been allocated here.

The mirror of that rule is that marking must not write on a foreign object
either. A list this backend made is marked by clearing its own `$u`; anything
else goes into a `WeakSet`, so `$share` hands a host array back exactly as it
received it. The set is also what holds the mark for a **struct, tuple or
enum**, none of which carry a bit: nothing writes into an aggregate — a
functional update spells its fields out or copies the array — so the only thing
an aggregate's sharing decides is what a field read out of it inherits.

#### The projection rule

Which is the last piece, and it is Perceus's drop specialisation with the answer
deferred. `state = State { ..state, items: state.items.push(x) }` is the
record-accumulator fold, and it has to stay in place or the whole exercise moves
the quadratic from the list to the struct around it. The field `state.items` is
a second reference to a list the struct still holds — *unless* this expression is
the last use of `state`, in which case the struct is about to have no readers
and its field is not shared by it.

`middle::rc` knows which of those it is, because last use is what its liveness
already computes; what it cannot know is whether `state` *itself* was shared
further up. So the compiler emits the question rather than the answer:
`$fromShared(state, state[1])` marks the field only if the parent is marked. A
`state` the caller kept was marked at the call, so the field is marked, so the
push copies — once, into a fresh unmarked struct, after which the loop runs in
place.

#### What the ownership half had to be told

Three things change in `middle::rc` under `Options::sharing`, and each is a place
where the native convention says something a garbage collector makes false:

1. **A growing list operation consumes its receiver.** Natively `list.push`
   borrows and `append_dest` tests the count; here there is no count, so the
   receiver must be owned and a caller that keeps the list duplicates it. That
   duplication is the mark.
2. **A lambda's body is scanned.** `middle::closures` does not run on this
   branch — an arrow function closing over its scope is what the engine wants —
   so there is no lifted function carrying a plan of its own. Its parameters are
   owned, and the runtime functions that call one mark every element they hand
   over, which is the convention that makes that true.
3. **The base of a functional update is not a duplication.** The projections the
   update reads out of the base keep it live across its own siblings, which the
   generic scan reads as a second reference. True of a count; false of a
   reference that is being taken over.

And one thing narrows: the classifier. The native question is "does this value
hold a counted allocation", which a `Str` and a function value both answer yes
to. The sharing question is "can this value reach a **list**", because a list is
the only thing anything writes into — a `Str` is an immutable JavaScript string
and a function value is a closure. `Syntactic::for_lists` is the same walk with
different leaves, and the difference is visible in the output: a `Point { x: Int,
y: Int }` and a `Result<Int, Str>` carry no marks at all.

The two questions also have opposite safe directions, which is why they are not
one function with a flag threaded through. A type the native walk cannot answer
gets no operations and leaks; a type this walk cannot answer gets marked, because
the failure on this side is an aliased list nobody copied. A recursive type is
the same story from the other end: the native walk says `Yes` at its depth bound
because a type that reaches itself is behind a pointer and therefore counted,
while "reaches a list" is a least fixed point and an expression tree that reaches
only itself and an `Int` reaches no list at all.

#### What it costs

The bit is a named property on a JavaScript array. That is the one thing worth
measuring rather than reasoning about, so it was measured against the
alternative — a wrapper object `{ a, u }` with the list inside it — on both
engines the suite runs under. Element reads, which outnumber everything else,
are identical across the marked array, the bare array and the wrapper (0.54 /
0.55 / 0.54 ms per million on JavaScriptCore; 0.99 / 1.00 / 1.01 on V8), and
mixing marked and unmarked lists through one call site costs nothing measurable
either — an array's elements do not live in its property backing store, so the
named property does not move them. Growing is a wash. The wrapper's only edge is
in stamping itself, and against that it would put a dereference on every list
access in the compiler and the runtime, and would need wrapping and unwrapping at
every host boundary — which is the boundary whose whole property is that a
foreign array is recognisable by carrying nothing. The named property wins.

Two million pushes cost the same whether they are two hundred runs of ten
thousand or twenty runs of a hundred thousand, which is what linear means and
what `tests/language/sharing.rs` asserts. The artifacts grow by about one per
cent, which is the two helpers and the branch in each of the six operations.

## 6. What this costs, honestly

Reference counting is not free, and the places it is not free in this language
are:

- **Shared, deeply nested, short-lived data.** Building a large JSON tree and
  dropping it walks the whole tree twice — once to build, once to free — where a
  generational collector would have dropped a nursery. `core/json`'s parser is
  the shape most exposed to this.
- **`Str` views keep their parent alive.** `s.splitOnce(",")` on a 10 MB string
  and keeping one 3-byte half retains all 10 MB. This is a real footgun and it
  is the price of `slice` being pure (`str.buri`). It is documentable rather
  than fixable — a copying `slice` would have to name `Alloc`, which is a
  language change — and `core/str` now says so where `slice` is declared.

  **Ruled on, and closed.** This was carried as a language question because the two alternatives — copying above a ratio, or
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
  `splitOnce` are declared without an `Alloc` bound (`str.buri`). The
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
  `Result<Region, _>` (`effect.buri`), so there is no value to report failure
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
`HostAlloc` is `struct HostAlloc {}` (`host.buri`) — zero-sized. So on the
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
weighs** (`stencil/runtime.rs`, `llvm/runtime.rs`). That is not an oversight to
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
