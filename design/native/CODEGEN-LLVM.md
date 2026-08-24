# The LLVM backend

The release backend: `buri build --release` for `Linux` and `Macos`
(ARCHITECTURE.md §4). It exists to do the one thing the middle end and Cranelift
between them do not — instruction selection, scheduling, register allocation and
vectorization at production quality.

Facts about LLVM and inkwell were checked against inkwell 0.10.0 (2026-08-06),
`llvm-sys` 211.x, and LLVM 21.1, which is what §8 pins.

## 0. The four instructions

Four instructions govern what a frontend owes LLVM, and this document is
organised around answering them. They are normative for every native codegen
document in `design/native/`, and the compiler cites them where it obeys them.

1. **Do dead code elimination before it reaches LLVM IR.** Answered in §1.
2. **Avoid `mem2reg`; generate optimized SSA form.** Answered in §2, and it is
   the reason the middle end has a block-argument IR at all
   (ARCHITECTURE.md §2.1).
3. **Supply comprehensive LLVM attributes** — `noalias`, `nounwind`, `readnone`,
   `readonly`, `nonnull`, `align`. Answered in §3, where the effect system turns
   out to supply most of them for free.
4. **Structure for cache locality and basic-block size.** Answered in §6.

They are stated as instructions rather than as goals because each is a thing a
frontend either did or did not do before LLVM was handed a module, and each has
a section below that says which.

## 1. Dead code elimination happens before this file runs

§0's first instruction. `middle::dce` (ARCHITECTURE.md §2.2) runs after inlining
and before lowering, so the module handed to LLVM contains no unreachable function
and no unused local. Three things make that stronger here than in most compilers:

- Monomorphization is reachability-driven, so an instance nothing calls is never
  created (`monomorphize.rs`) — the standard library costs nothing in a
  program that touches two functions of it.
- The call graph is exact: no dynamic dispatch anywhere in the language
  (`monomorphize.rs`), so "unreachable" is a fact, not an approximation.
- Descriptors do not reach the artifact (VALUE-MODEL.md §9), which is what stops
  every type in the program from being transitively reachable from a runtime
  walker.

The consequence for LLVM is measurable rather than aesthetic: a module with no
dead code is a module `globaldce` and the inliner walk over quickly, and the
per-unit compile time in `--release` is what dominates a release build.

## 2. Direct SSA, and no `alloca`

§0's second instruction: avoid `mem2reg`, generate optimized SSA form.

### 2.1 Block parameters become phis, mechanically

`middle::ir` is block-argument SSA (CODEGEN-CRANELIFT.md §1). Block parameters and
phi nodes are the same construct in two notations, and the transliteration is:

| `middle::ir` | LLVM |
|---|---|
| `Block { params: [(v, t), ...] }` | one `phi t` per parameter, at the top of the basic block |
| `Term::Jump(b, args)` | `br label %b`, and each `args[i]` becomes an incoming pair on `b`'s *i*th phi |
| `Term::Branch { cond, then, else_ }` | `br i1 %cond, label %then, label %else` plus incoming pairs on both sides |
| `Term::Switch { on, cases, default }` | `switch` |
| `Term::Return(vs)` | `ret`, aggregating multiple results into a literal struct |

Phis are built in two passes: create every `BasicBlock` and every phi with the
right type but no incoming values, then fill bodies, then add incoming values
once every predecessor's terminator has a `BasicBlock` to name. inkwell's
`PhiValue::add_incoming(&[(&dyn BasicValue, BasicBlock)])` takes them in any
order.

There is no critical-edge splitting to do: a block parameter carries a *distinct*
argument list per edge by construction, so the two-predecessors-with-different-values
case that forces edge splitting in a mutable-slot IR cannot arise.

### 2.2 So mem2reg has nothing to promote

Nothing in the lowering emits `alloca` for a local, a parameter, a temporary, a
match binding, or a loop variable. `SROA`/mem2reg still runs as part of
`default<O2>` (§4) and is cheap when there is nothing to promote — it walks the
entry block's allocas, finds none, and returns.

The counter-design — emit an `alloca` per local, `store` on definition, `load` on
use, and let mem2reg sort it out — is what most hand-written LLVM frontends do
and it is what §0's second instruction names. Its cost is not just the pass: it is that
every intermediate value passes through memory in the IR the *inliner* and the
early simplification passes see, so the decisions those passes take are taken on
a worse program.

### 2.3 The one place `alloca` is emitted, and why it is not a violation

Escape analysis (MEMORY.md §5.2) promotes a struct that is constructed and
consumed within one function, never stored into a heap value and never returned,
into stack memory. That is an `alloca` in the entry block.

It is not the case §0's second instruction is about. The instruction is about using
memory as a stand-in for SSA values — an `alloca` whose whole purpose is to be
promoted back. This one is a genuine aggregate in memory whose fields are
accessed by `getelementptr`, exactly as a C local struct would be, and SROA
splitting it into scalars afterwards is the optimization doing its job rather
than cleaning up after the frontend. It is emitted in the entry block, which is
what makes it eligible for that treatment at all.

### 2.4 There is no mutable-slot emulation, including for tail-call loops

The obvious place a frontend needs mutable slots is the loop that
`middle::tail_calls` produces from a self-recursive tail call: the parameters are
rebound each iteration, and in JavaScript that is literally assignment
(`generate.rs`, `retarget_assigns`).

In block-argument SSA it is not assignment. The loop header block takes the
parameters as block parameters, and "rebind and continue" is a `Jump` back to the
header with the new values as arguments — which is a phi, in the right place,
with no slot involved. The same holds for the merged dispatch function of a
mutually-recursive group (`tail_calls.rs`), whose dispatch variable is one
more block parameter carrying the member index.

So the construct that would have forced mutable-slot emulation is the construct
that most naturally produces phis, and there is nothing to emulate. That
falls out of `middle::tail_calls` doing the rewrite rather than the backend doing
the emission (ARCHITECTURE.md §2.2), which was the reason to move it.

## 3. Attributes

§0's third instruction. The effect system supplies most of this for free, and it is
worth saying so plainly: **a language where "does this function touch the world?"
is a syntactic property of its signature is a language that can answer LLVM's
memory-effect questions without an analysis.** SPEC 10.4's purity theorem is an
attribute-emission rule.

### 3.1 What comes from the effect system

> **Purity theorem.** If a function has no `ctx` parameter, no effect-carrying
> `self`, captures no effect-carrying value, and constructs no context, then any
> two evaluations on identical arguments that terminate without aborting, in the
> absence of undefined behaviour, produce identical results and perform no
> observable effect. (SPEC 10.4)

That is `memory(none)` plus `willreturn`, with two conditions attached — and both
conditions are the qualifiers the theorem already names, which is why they are
load-bearing rather than decorative:

- **"terminate without aborting"**: an abort writes to stderr and exits, which is
  an observable effect. So `memory(none)` requires that the function cannot
  abort. `middle` computes this: a function aborts if it divides by a value not
  proved non-zero, or calls one that can. Where it can, the attribute is
  `memory(argmem: read, inaccessiblemem: write)` — it reads its arguments and may
  write to a place the caller cannot observe except by not returning.
- **"in the absence of undefined behaviour"**: overflow is undefined (SPEC 6.2),
  and §3.4 declines to tell LLVM so.

The table:

| Buri property | LLVM attribute |
|---|---|
| No `ctx`, no effect-carrying `self`, reads no heap through a parameter, **adjusts no reference count** | `memory(none)` |
| Same, but reads through a parameter (every `[T]`, `Str`, struct pointer, and every tagged enum's payload blob) | `memory(argmem: read)` |
| Same, and can abort | add `inaccessiblemem: write` |
| Same, and **adjusts the count of a value based on a parameter** | `argmem` becomes `readwrite` — §3.2.1 |
| Same, and adjusts a count reached any other way | no memory attribute for the heap: the default location is `readwrite` |
| Bounded only by `Alloc` — deterministic (SPEC 10.5) | `memory(write, argmem: read, inaccessiblemem: readwrite, errnomem: readwrite)` — §3.2.1's second half |
| Bounded by an observable effect | no memory attribute; the default `memory(readwrite)` |
| Every function, on every backend | `nounwind` — there is no unwinding in the language at all (SPEC 6.10) |
| A function `middle` proved terminates | `willreturn`, and `mustprogress` |
| `memory(none)` + `willreturn` + `nounwind` | add `speculatable` — a call may be hoisted above a branch |
| Every function that cannot abort | `nofree` is *not* set: `decref` frees |

The purity theorem is still an attribute-emission rule and this is still where
most of the discipline comes from for free. What the fourth and fifth rows add
is the one thing the theorem does not talk about: SPEC 10.4 is a statement
about *observable effects*, and LLVM's `memory(...)` is a statement about
*bytes*. Reference counting writes bytes that no Buri program can observe, and
that difference is §3.2.1.

`nounwind` on every function is the single most valuable one and it costs no
analysis: the language has no `throw`, no `panic` that unwinds, and no
`catch`. An abort is `write` then `_exit` (`generate.rs` is the
JavaScript spelling of the same contract). LLVM without `nounwind` has to assume
every call is a potential unwind edge, which pessimizes everything downstream of
it.

### 3.2 What comes from the value model

| Buri property | LLVM attribute |
|---|---|
| Every pointer parameter that is not a niche-encoded `Option` (VALUE-MODEL.md §6) | `nonnull` on the parameter |
| Every heap pointer (16-byte aligned header, VALUE-MODEL.md §2) | `align 16` on the parameter and the return |
| A pointer parameter whose block this function adjusts no count in | `readonly` — §3.2.1 for the condition, which is not "always" |
| A parameter whose lifetime the caller guarantees for the call (MEMORY.md §5.2's *borrowed*) | `nocapture` |
| A pointer parameter the callee does not alias with any other parameter | `noalias` — §3.3 |
| Every `Str`/`[T]`/heap pointer return | `noalias` on the return, plus `align 16` |
| A parameter the layout says is `Bool` | `range(i8 0, 2)` |
| A `Char` parameter | `range(i32 0, 0x110000)` |

Note that `readnone`/`readonly` remain valid **parameter** attributes in current
LLVM; it is the *function*-level spellings that were replaced by `memory(...)`.
§3.5 has the encoding hazard.

#### 3.2.1 The reference count is memory

This section used to say that `readonly` on a parameter is "true
unconditionally and would be a lie in almost any other language", on the
argument that values are immutable, that `middle::rc`'s in-place growth
(MEMORY.md §5.3) writes only past the end of a block whose count it has just
seen to be 1, and that the count itself is not part of the value. **Every one of
those statements is true about Buri and none of them is what `readonly`
means.** LangRef:

> `readonly` — This attribute indicates that the function does not write
> through this pointer argument [...] If a function writes to a readonly
> pointer argument, the behavior is undefined.

> `memory(...)`, access kind `read` — The location is only read. Writing to the
> location is immediate undefined behavior. **This includes the case where the
> location is read from and then the same value is written back.**

That last sentence removes the "unobservable write" defence entirely: `read` is
a promise about bytes, and a store of the identical value is still a store. And
LangRef's *Pointer Aliasing Rules* settle where the count lives — a pointer
formed by `getelementptr` **is** *based on* its operand, `argmem` is "accesses
that are based on pointer arguments", so `incref`'s store at `p - 16` for a
parameter `p` is a write to argument memory. Three writes in this language go
through a parameter:

- `incref`'s store of the count at `p - 16` (MEMORY.md §5.1);
- `decref`'s store of the count, and its `free` of the block;
- MEMORY.md §5.3's in-place growth — the `memmove` past the end of `concat`'s
  left operand, and `cli/runtime/list.rs`'s `append_dest`.

Because a function's `memory(...)` covers its callees' effects as well as its
own, a caller of a function that increfs is a function that increfs, so the
condition is a whole-call-graph one. `backend/llvm/emit.rs`'s `observe` computes
it beside the allocation and abort bits it already propagated, and
`argument_based` is the provenance analysis that decides *which* memory a count
sits in: a register projection of a parameter keeps the parameter's provenance,
a `load` — a `[T]`'s element — does not, and a block parameter takes the
conjunction of its incoming values, which matters for every counted function
because `middle::tail_calls` turns self-recursion into a loop.

The corrected rule, which is what `backend/llvm/attrs.rs` implements:

| what the body does to a count | `readonly` on the parameter | `memory(...)` |
|---|---|---|
| nothing | yes | `argmem: read` |
| counts a value *based on* a parameter | **no** | `argmem: readwrite` |
| counts anything else | yes — it is not a write through *this* parameter | the default location becomes `readwrite` |

None of this is a guess about what LLVM will tolerate. `opt
-passes=function-attrs` on the same shape — a `getelementptr i64 -16` from a
parameter, a load, an add, a store — infers exactly `memory(argmem: readwrite)`
and `captures(none)` **without** `readonly`, and on the third row it infers
`readonly` on the parameter and `memory(readwrite, inaccessiblemem: none)`.
LLVM's own answer and this table are the same table.

The same reading corrects two neighbours:

- **The allocating row.** A function that allocates and then initializes the
  block writes memory that is neither argument memory nor inaccessible: once
  `buri_rt_alloc` has returned, the block is ordinary program memory, and
  LangRef's parenthetical about an allocator returning newly accessible memory
  excuses the *allocator*, not its caller. `opt` agrees — it infers
  `memory(write, argmem: none, inaccessiblemem: readwrite)` for exactly that
  body, because the block escapes through the return and so gets no
  function-local carve-out. `errnomem` comes with it: `malloc` sets `errno`.
- **A tagged enum's payload parameter.** `repr.rs` gives it `SlotTy::Blob`, an
  *integer*, so a signature of one used to narrow to `memory(none)` for having
  no pointer parameter. A `Result<Str, E>` keeps its `Str`'s three words inside
  that integer and gets them back with an `inttoptr`, which LangRef's aliasing
  rules make based on the parameter. A blob parameter is a pointer parameter
  wearing an integer's type, and only a genuinely all-scalar signature narrows.

What survives is the whole of the discipline for functions that touch no count,
which on a representative program is most of them: of twenty pointer parameters
across twelve functions, sixteen carried `readonly` before and ten after — the
six that went are exactly the parameters of the three functions with a
reference-counting plan — and no function lost its `memory(...)` attribute
outright.

`cli/tests/native/llvm.rs` holds the two tests that keep this true. One scans
*every* emitted function for the pattern rather than asserting on one of them:
a store to `p - 16` for a `p` the define line marks `readonly`, or under a
`memory(...)` whose `argmem` is not writable. The other runs one program
through `default<O0>` and `default<O2>` and requires the two to agree about
stdout, the exit code and `buri_rt_live_blocks` — which is what a memory
attribute being exploited would break.

### 3.3 `noalias`, and where it stops

`noalias` on a parameter promises the callee that objects reached through it are
reached through no other argument. That is *not* generally true here: two `[T]`
parameters may be the same list, and two `Str` parameters may be views into one
buffer (VALUE-MODEL.md §3).

It is true in the cases that matter, and `middle` proves them:

- **A freshly allocated return value.** Every allocating runtime entry returns a
  block nothing else has a reference to. `noalias` on the return of
  `buri_rt_list_map`, `buri_rt_str_concat` and every constructor is
  unconditional (the prefix is `buri_rt_`, always — VALUE-MODEL.md §10) and is the single most
  valuable use of the attribute, because it is what lets LLVM keep a just-built
  aggregate's fields in registers across a call.
- **A parameter of a function that has exactly one pointer parameter.** Vacuous
  and free.
- **A parameter that `middle::rc` proved uniquely owned** (`rc == 1` at the call).
  This is the reuse analysis (MEMORY.md §5.3) paying a second dividend.

Everywhere else, no `noalias`. Emitting it where aliasing is possible is a
miscompile that shows up as a wrong answer months later, and this is a language
whose whole pitch is that wrong answers are caught early.

### 3.4 What is deliberately *not* emitted

**`nsw` and `nuw` are never set on integer arithmetic.** SPEC 6.2 says overflow is
undefined behaviour, and `nsw` is exactly how LLVM spells that, so setting it
would be *correct*. It is still not set, for a reason that is about the language
rather than about LLVM:

VALUE-MODEL.md §11 records the SPEC 6.2 amendment describing two backends whose overflow
behaviour differs — precision loss on JavaScript, two's-complement wrap natively.
"Two's-complement wrap" is a description a program can be debugged against.
`nsw` makes it "whatever the optimizer inferred from the assumption that it never
happened", which is a description nothing can be debugged against, and which
changes between LLVM releases. A program that overflows is wrong either way; one
of the two answers can be reported in a bug and reproduced, and the other cannot.

The cost is real and is accepted: LLVM loses some induction-variable widening and
some loop-bound reasoning on `i32` loops. It is small in a language whose default
integer is already `i64`, which is the pointer width, so the most common case
needs no widening at all.

**`inbounds` on `getelementptr`** *is* set, everywhere, without exception. Every
projection in the language is either a field of a struct whose layout is known or
an index that `list.get` bounds-checked into an `Option` (`list.buri`:
there is no way to index out of bounds). Unlike `nsw`, the premise is enforced by
the type system rather than assumed away.

### 3.5 The `memory(...)` encoding hazard

`memory` is an `IntAttr`, so inkwell reaches it through the ordinary enum path:

```rust
let kind = Attribute::get_named_enum_kind_id("memory");   // assert non-zero
let attr = ctx.create_enum_attribute(kind, bits);
f.add_attribute(AttributeLoc::Function, attr);
```

`bits` is a `MemoryEffects` bitmask — two bits per location, `NoModRef=0, Ref=1,
Mod=2, ModRef=3` — and **the location list changed twice in the versions this
project could plausibly build against**:

| | locations | `memory(none)` | `memory(read)` | `memory(readwrite)` |
|---|---|---|---|---|
| LLVM 18-20 | ArgMem, InaccessibleMem, Other | 0 | 0x15 | 0x3F |
| LLVM 21 | + ErrnoMem | 0 | 0x55 | 0xFF |
| LLVM 22 | + TargetMem0, TargetMem1 | 0 | 0x555 | 0xFFF |

`memory(none)` is 0 in every version and the argmem-only forms are stable;
everything that names the *default* location is not. So the bitmask is
constructed in exactly one place — `backend/llvm/attrs.rs`, a small
`MemoryEffects` builder gated on the LLVM version — and no call site anywhere
writes a literal. A hard-coded `0x55` is a silent miscompilation of the attribute
on an LLVM bump, which is the worst kind: the IR still verifies.

## 4. The pass pipeline

`module.run_passes("default<O2>", &target_machine, options)`.

**`default<O2>`, not `default<O3>`.** O3 differs mainly in unroll and vectorize
aggressiveness and in inline thresholds; it costs code size and compile time for
a gain that is workload-dependent and usually small, and O2 is what production
toolchains ship by default for the same reason.

**A pipeline string, not a hand-assembled pass list.** A custom pipeline is a
list that has to be re-derived and re-tuned at every LLVM bump, against a pass
manager whose pass names and orderings are internal. `default<O2>` is the
pipeline LLVM's own developers regression-test. The only defensible reason to
hand-assemble one would be that the middle end has made some pass redundant, and
a redundant pass on a module it has nothing to do costs approximately nothing.

`PassBuilderOptions`:

| Setting | Value | Why |
|---|---|---|
| `set_merge_functions` | `true` | Monomorphization produces structurally identical functions at different types — `Option<A>` and `Option<B>` where both are pointer-sized, every `[T]` helper at two pointer-shaped `T`s. Merging them is free size. |
| `set_loop_unrolling` | default (on) | |
| `set_loop_vectorization` | default (on) | The point of `--release` on `[Int]` folds. |
| `set_verify_each` | `cfg!(debug_assertions)` | Checks our IR, not LLVM's; the same rule as Cranelift's verifier (CODEGEN-CRANELIFT.md §4). |

The legacy `PassManager` is deprecated in inkwell 0.9.0 and is gated
`#[llvm_versions(..=16)]` — it does not exist at all on LLVM 17 and above, so
there is no choice to make and no migration to plan. `Module::run_passes`
requires a `&TargetMachine`, so the target is initialized before any optimization
even in a hypothetical IR-only run.

`FunctionValue::run_passes` exists on LLVM 20+ and is not used: the unit is the
module (ARCHITECTURE.md §5), and per-function pipelines would lose the
module-scoped passes that the unit boundary is already narrow enough to cost.

## 5. Tail calls, and the analysis behind not using `musttail`

The instruction is: the middle end already converts self and mutual tail calls to
loops, so LLVM gets loops, and `musttail` is only needed for indirect and
cross-function cases. The analysis, and the conclusion, is that **`musttail` is
not needed at all** and neither is `tailcc`.

**What the middle end delivers.** `middle::tail_calls` rewrites a self-recursive
tail call into a loop and a mutually tail-recursive SCC into one dispatching
function (`tail_calls.rs`). Both are exact — "the emitted loop is what a
hand-written loop would have been" (`tail_calls.rs`) — and both reach LLVM as
ordinary CFG loops with phis (§2.4).

**What is left, and why it needs nothing.** A tail call to a function *outside*
the caller's tail-call SCC. After SCC merging the tail-call graph is a directed
acyclic graph, so any chain of such calls has length bounded by the DAG's longest
path — a compile-time constant, typically two or three. The stack does not grow
without bound, so SPEC 8.3 is satisfied without a guaranteed tail call anywhere.
That argument is backend-independent, which is why the JavaScript backend has
never needed one either.

**What `musttail` would cost.** It is guaranteed-or-fatal: a backend that cannot
honour it calls `report_fatal_error`, so an LLVM version or a target that
tightens a rule turns a working program into a compiler crash. On AArch64 —
half the platforms here — `musttail` does *not* bypass
`isEligibleForTailCallOptimization` and hard-errors on `byval` parameters, on
SME/streaming mode changes, and when outgoing stack arguments exceed the caller's
incoming area. There is an open AArch64 miscompile in exactly this combination
(`llvm#167181`, `musttail call tailcc` under BTI+PAC leaving SP unrestored).

**What `tailcc` would cost.** It relaxes most of the AArch64 conditions, and it
is **viral**: the calling convention must match numerically at every call site,
so adopting it for one function adopts it for its entire call graph. `cli/runtime`
is a C-ABI archive (VALUE-MODEL.md §10), so the whole 203-function intrinsic
surface would need C-ABI shims. `tailcc` also forbids varargs entirely, and is
not the platform C ABI, so a Buri artifact could no longer be linked against
anything.

**The decision.** `fastcc` for Buri-to-Buri calls, `ccc` for `buri_*` runtime
entries, and the plain `tail` marker (`CallSiteValue::set_tail_call(true)`) on
calls in tail position as a *hint* LLVM may honour or ignore. `fastcc` costs
nothing — both sides of every Buri call are generated here, there is no ABI to be
compatible with, and VALUE-MODEL.md §5.1 already flattens aggregates so no
convention has to classify one.

inkwell has no `CallConv` enum; the convention is a raw `u32` set on both the
function (`set_call_conventions`) and every call site (`set_call_convention`), and
a mismatch between the two is a miscompile LLVM will not diagnose. One helper
sets both, and nothing else in the backend names a convention number.

**The one real gap**, named so it is not rediscovered: an *indirect* tail call —
a closure tail-calling itself through a value — is not eliminated on any backend,
because `tail_callees` collects only `ExprKind::CallFn` (`tail_calls.rs`). It
is a middle-end gap that predates this design, and the fix is a middle-end one.

## 6. Cache locality and basic-block size

§0's fourth instruction.

- **Cold blocks are marked cold.** Every abort path, every `decref` free path
  (MEMORY.md §5.1), every `.None` arm of a `?` (SPEC 6.8), and every
  `defensive_aborts` block gets `cold` on the call and `unreachable` after it
  where it does not return. That is what moves them out of the hot path in block
  placement, and it is the highest-value item on this list because reference
  counting puts a rarely-taken branch next to *every* value that dies.
- **Blocks are not split more than the IR requires.** The decision tree
  (ARCHITECTURE.md §2.2) produces one block per distinct outcome, not one per
  test, so a six-variant match is one `switch` and six blocks rather than six
  compare-and-branch blocks. This is a direct improvement on the current
  arm chain (`generate.rs`), where a six-variant enum performs six
  comparisons to reach its sixth arm — `generate.rs` says so.
- **The unit is a source module** (ARCHITECTURE.md §5.1), so functions that call
  each other land in one `.text` section next to each other. Within a unit,
  emission order is the middle end's function order, which is the
  monomorphization worklist's — deterministic, and derived from the reachability
  walk out of the entry point rather than from a hash order
  (`monomorphize.rs`). That is a free first approximation of a
  call-order layout: a callee is instantiated when its caller is reached, so
  related functions are adjacent. A measured layout, from `--explain` counts or a
  profile, is a later item and would replace only the ordering, not the
  partition.
- **`llvm.expect` is not emitted.** There is no branch-probability information in
  the source language and no profile, so any hint would be invention. `cold` is
  emitted only where the block is provably exceptional.

## 7. Debug info

DWARF via inkwell's `DebugInfoBuilder`, in the release backend, from wave 3
onward. It is cheaper here than in Cranelift (CODEGEN-CRANELIFT.md §5) because
LLVM emits the sections; the frontend supplies `DILocation`s from
`typed::Expr::span`, which every node already carries (`typed.rs`), and
`DISubprogram`s from `Func::debug_name` (`monomorphize.rs`).

Two constraints from ARCHITECTURE.md §7: `DW_AT_comp_dir` is the repository root
spelled relatively, and on Mach-O the `N_OSO` entries name repository-relative
object paths. Both are set from `Options::unit_prefix` rather than from
`std::env::current_dir`, because `--check-reproducible` builds into two different
directories precisely to catch the version that does not.

The prefix does not wait for that wave to reach an object. It is already the
head of a unit's module name (`emit_selected`), and LLVM emits a module's
source-file name as a `.file` directive on every target whose assembly syntax
has one — every ELF target, no Mach-O one — so it lands in `.symtab` as an
`STT_FILE` symbol today. That is why it is a term of the `codegen` key
(ARCHITECTURE.md §6.2).

## 8. Version pinning

```toml
[dependencies]
inkwell = { version = "0.10", optional = true, features = ["llvm21-1"] }
```

**LLVM 21.1, inkwell 0.10, `llvm-sys` 211.x, `LLVM_SYS_211_PREFIX`.**

### 8.1 The policy: exactly one LLVM, tracking latest

Ruled on, and it settles what `OPEN-QUESTIONS.md` asked:

- **Exactly one supported LLVM at any moment.** Not a range, not a minimum, not a
  set of `#[cfg]`-selected encodings. `strict-versioning` below is what would
  enforce that in the build rather than by a promise, and §8.3 records why it is
  not on yet.
- **The pin is the latest LLVM that inkwell *and* the flake's nixpkgs both
  carry.** Both, because a pin either side cannot supply is a pin nobody can
  build: inkwell is where the feature flag comes from and nixpkgs is where the
  `llvm-config` comes from. Where they disagree, the older wins, and today they
  disagree — inkwell has `llvm22-1` and `nixos-25.05` stops at 21.
- **The version is an internal detail.** No BUILD file, no `REPO.buri`, no
  `Output`, no diagnostic and no flag names an LLVM version. What a program can
  observe is `Backend::identity()`, which names the linked LLVM *because* a
  codegen change must invalidate cached objects (ARCHITECTURE.md §3) — that is a
  cache key, not an interface.
- **Bumping it is a chore, not an event.** No deprecation window, no transition
  period, no compatibility shim. The checklist is in §8.2 and it is four lines
  plus a test.
- **Multi-version support is refused, permanently.** The `memory(...)` bitmask in
  §3.5 is why: supporting two LLVMs means the *same* attribute encoding two ways
  and a test matrix that has to build both, for a benefit — a contributor keeping
  an older LLVM — that `nix develop` already delivers.

Neither of the two policies `OPEN-QUESTIONS.md` posed survives as written: "the
flake leads" is right about the *default* and wrong as a rule, because it would
forbid a bump the backend actually needs, and "the backend leads" prices a
nixpkgs bump — which moves `cargo`, `bun` and `elan`, and therefore every
artifact this toolchain produces — into a routine chore. The rule is that the pin
tracks latest-available, so it moves when a nixpkgs bump that was happening
anyway brings a newer LLVM along. A bump *for* an LLVM is possible and is a
nixpkgs decision made on nixpkgs' terms, with the cache invalidation priced in,
and no codegen improvement has yet been worth asking for one.

### 8.2 The bump checklist

Four edits and a test, in this order:

1. `cli/Cargo.toml` — the inkwell feature, `llvm21-1` → `llvm<N>-1`, and the
   `prefer-dynamic` variant beside it.
2. The `LLVM_SYS_211_PREFIX` environment variable name — it carries the major and
   minor, so `LLVM_SYS_<N>1_PREFIX`.
3. `flake.nix` — `pkgs.llvmPackages_21` → `pkgs.llvmPackages_<N>`, which is what
   makes the two agree.
4. `backend/llvm/attrs.rs` — the `Location` list, per §3.5's table.

Then `the_bitmask_matches_llvm_21s_location_list` in `attrs.rs` is the canary:
it asserts `MemoryEffects::everything().bits()`, which changes at every LLVM
whose location list changes, so a bump that forgot step 4 fails there rather than
miscompiling an attribute that still verifies. Rename it with the version. The
`identity()` change falls out of the linked version and invalidates cached
release objects, which is correct.

### 8.3 Why 21 today

inkwell supports LLVM 12 through 22 (`llvm22-1` is the newest flag) and currently
has zero lag behind stable LLVM. LLVM 22 would be the newest valid pin on
inkwell's side alone. 21 is what both sides carry:

- `nixpkgs` on the pinned `nixos-25.05` provides `llvmPackages_9` and `_12`
  through `_21`; `_21` is 21.1.2 and `llvmPackages` (the default) is 19.1.7.
  **There is no `llvmPackages_22` on 25.05** — re-checked against the locked
  revision, not against the channel name. So 21 is the latest the two sides
  share, which is what §8.1 asks for. Pinning 22 would mean bumping the flake's
  nixpkgs before the backend can be built at all, which is a change to how the
  whole toolchain is built in service of a codegen decision.
- LLVM 21 is nixpkgs' *default* `llvmPackages` on 25.11, 26.05 and unstable, so a
  contributor on any of those gets it without naming a version.
- Homebrew ships `llvm@21` (21.1.8) for macOS contributors who are not on nix.

What 22 would buy is `musttail` on RISC-V, which §5 declines to use on any
target — so there is nothing to want, and the pin moves when the flake does,
through §8.2.

**`strict-versioning` is asked for and is not on.** `llvm-sys` will otherwise
build against any LLVM at least as new as its target, so a machine with LLVM 22
installed would silently produce a toolchain that generates different code from
one built against 21, and for a compiler whose central claim is byte-identical
output "at least as new" is not a version policy. What blocks it is the
dependency bar rather than the argument: inkwell's `llvm21-1` expands to
`llvm-sys-211` and nothing else, so reaching the flag would mean a second direct
dependency on `llvm-sys` for one setting. Standing in for it until that is
decided: `Backend::identity()` puts the LLVM version into every release
`codegen` key, so two toolchains built against different LLVMs never share a
cached object, and `llvm-config --version` is what a contributor checks.
`cli/Cargo.toml` and BUILD-AND-WATCH.md §3.4 both say so where the decision is
made.

**Linking mode.** `llvm-sys` defaults to `prefer-static` since 191. On macOS
there is an open bug (`llvm-sys` GitLab #80): `llvm-config --system-libs
--link-static` emits Homebrew static archives mixed with shared libraries and
`build.rs` panics with "Shared library should be a .so file". The macOS build
therefore uses `inkwell/llvm21-1-prefer-dynamic`, selected by a target-gated
feature, and Linux keeps the static default. The linking-mode features must be
selected through inkwell rather than through `llvm-sys` directly — a bare
`llvm-sys` feature makes Cargo try to resolve every LLVM version.

**The nix devShell** gains, on top of what it has:

```nix
llvm = pkgs.llvmPackages_21.llvm;
...
packages = [ ... llvm.dev llvm pkgs.libxml2 pkgs.libffi pkgs.lld pkgs.mold ];
LLVM_SYS_211_PREFIX = "${llvm.dev}";
```

`llvm.dev` and not `llvm`: the `.dev` output is the one carrying
`bin/llvm-config` and the headers, and pointing at the default output fails in a
way whose error message does not say so. `zlib` is already in the shell;
`zstd` and `ncurses` are added if `llvm-config --system-libs` asks for them on a
given configuration, which varies. `mold` is Linux-only (2.39.1 on 25.05) and is
listed conditionally; `lld` follows the default `llvmPackages`, so it is 19.1.7 on
25.05 — which is fine, because the linker's version does not have to match the
compiler's.

**Not a build-time requirement for everyone.** `backend-llvm` is off by default
(BUILD-AND-WATCH.md §2), so `cargo install buri` needs no LLVM at all.
