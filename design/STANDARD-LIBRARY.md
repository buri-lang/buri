# Standard library: the decisions behind it

The user-facing map of `core/*` — which modules exist, what each costs, and
what is absent — is `cli/src/docs/reference/standard-library.md`, served as
`buri docs reference/standard-library`. The reference for any module is the module
itself (`buri docs std core/list`, rendered from the source the compiler
checked).

This file holds the half of that document a user does not need: why the
absences are absences, what would have to change to close them, and the rules
anything added here has to satisfy.

## 1. Allocators, and why they arrived when they did

`GeneralPurpose`, `Arena` and `FixedBuffer` were deliberately deferred while
the only backend had a garbage collector, on the grounds that a count would be
synthetic.

**What made them real was not the native backend but the cost model.** The
charge for an operation is *defined* — a `Str` of *n* UTF-8 bytes charges
`16 + n`, a `[T]` of *n* charges `16 + n * stride(T)`, a view charges nothing —
and it is computed from the types rather than measured. So the same program
charges the same number on both backends by construction, and a count is not a
JavaScript fact that a native run would contradict. The model lives beside
`Alloc` in `core/effect`, where a reader of the effect meets it;
`design/native/MEMORY.md` §7 is the argument for it.

`FixedBuffer` aborting rather than answering an error is forced by the
signature: `allocate` answers `Region` and not `Result<Region, _>`, so there is
no value to report a failure with, and SPEC §6.9 says that is what an abort is
for.

## 2. The counted set is narrower than the modelled set

**What is counted is every `allocate(ctx, n)`, and nothing else** — identically
on both backends. The list and string rows are charged by definition and
reported to no allocator, because the allocating intrinsics drop the context:
on JavaScript because `stride(T)` is a compile-time fact an untyped runtime
does not have, and natively because the `buri_rt_*` ABI drops a context
argument from every call.

Widening that has to happen on both backends at once or the numbers stop
agreeing, which is the one property the module exists to have. It is therefore
a wave of its own, not an incremental fix. `core/alloc`'s own comment states
the boundary, and `cli/tests/conformance/lib/memory/` pins it on both backends.

## 3. Struct-of-arrays, in full

Not expressible, and not honourable on the JavaScript backend:

1. A struct is already an array in the JavaScript representation, so `[Point]`
   is an array of arrays. A columnar `{ xs: [Float], ys: [Float] }` really is
   faster in a JIT — but a user gets that today by writing the two-field struct
   themselves, and a library adds nothing.
2. A generic `MultiArrayList<T>` is not typeable. Exposing "column *i* of `T`,
   at `T`'s *i*-th field type" needs dependent or row types, and SPEC §5.5 has
   no records.
3. The version that would work is a type-generating `derive` — `derive Soa for
   Point;` producing a `PointSoa` and its accessors. Today `derive` only
   attaches conformance; generating a *new type* is a language change, and it
   belongs beside the native backend, which is where the layout would actually
   pay.

## 4. Bulk reclamation

The `Arena` type is here and its counter is real; the bulk free is not, and it
is not a backend gap. What would make it real is a **scoped context**: a
language feature bounding a context's lifetime so that everything allocated
under it is unreachable at the end of the scope. Until that exists an arena has
no scope to end at, so what it is for meanwhile is attribution — an arena per
phase, without subtracting two totals.

That is a language proposal, and it is tracked with the others rather than
here.

**Amendment (G4): the scope exists, and it reclaims reservations.**
`core/alloc`'s `scoped(ctx, body)` hands the body a `Scoped<C>` whose `Alloc`
is a fresh arena and whose every other effect forwards, and the arena's pages
are `munmap`ed when `body` returns. So the paragraph above is half answered:
`Arena` is still attribution, and a *scope* now reserves and releases.

**Amendment (G5): it reclaims the values too.** The gap G4 named was that the
native ABI drops the context argument from every runtime call, so the operation
that builds a list cannot be *told* which allocator asked for it (§7.3.1 of
`design/native/MEMORY.md`). It is not told: `scoped` makes its arena the one the
platform allocator serves from, for that carrier and for the dynamic extent of
`body`, and every block allocated in the extent is the scope's.

The other half is the deep copy that half-answer already named. Exactly one
value leaves a scope — `body`'s answer — and `core/alloc::copyOut` duplicates
every counted block reachable from it onto the caller's allocator before the
pages go back, at every depth: a nested `[[Str]]`, an enum's payload, a
closure's captured environment. A copy is not a share, so the answer points at
nothing the arena mapped and the bulk free cannot dangle.

So the paragraph this section opens with is answered rather than half answered.
`Arena` is still attribution — it has no boundary to copy at — and a **scope** is
now a lifetime for the values built inside it.

## 5. Two rules for anything added to `core/*`

1. **Every body-less declaration needs a conformance test that calls it.**
   `cli/tests/language/standard_library.rs` stops after type checking, so a declaration with no
   runtime function behind it passes that suite silently and fails only when a
   real program reaches it. The suites under `cli/tests/conformance/lib/` are
   what actually run the code.
2. **State the cost.** Every structure here is persistent, and persistent
   structures have costs that mutable ones do not. Saying `set` is O(n/32) is
   better than implying the O(1) a mutable bit set would have. The cost goes on
   the user-facing page, in the table, not in a comment.
