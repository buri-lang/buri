# The LLVM backend

The release backend: `buri build --release` for `Linux` and `Macos`
(ARCHITECTURE.md §4). It exists to do the one thing the middle end and Cranelift
between them do not — instruction selection, scheduling, register allocation and
vectorization at production quality.

`LLVM-tips.md` is normative for this document, and its four lines are answered in
order: dead code elimination before LLVM IR is §1; optimized SSA without mem2reg
is §2; comprehensive attributes is §3; cache locality and basic-block size is §5.

Facts about LLVM and inkwell were checked against inkwell 0.10.0 (2026-08-06),
`llvm-sys` 211.x, and LLVM 21.1, which is what §8 pins.

## 1. Dead code elimination happens before this file runs

`LLVM-tips.md:1`. `middle::dce` (ARCHITECTURE.md §2.2) runs after inlining and
before lowering, so the module handed to LLVM contains no unreachable function
and no unused local. Three things make that stronger here than in most compilers:

- Monomorphization is reachability-driven, so an instance nothing calls is never
  created (`monomorphize.rs:12-14`) — the standard library costs nothing in a
  program that touches two functions of it.
- The call graph is exact: no dynamic dispatch anywhere in the language
  (`monomorphize.rs:8-10`), so "unreachable" is a fact, not an approximation.
- Descriptors do not reach the artifact (VALUE-MODEL.md §9), which is what stops
  every type in the program from being transitively reachable from a runtime
  walker.

The consequence for LLVM is measurable rather than aesthetic: a module with no
dead code is a module `globaldce` and the inliner walk over quickly, and the
per-unit compile time in `--release` is what dominates a release build.

## 2. Direct SSA, and no `alloca`

`LLVM-tips.md:2`: avoid mem2reg, generate optimized SSA form.

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
and it is what `LLVM-tips.md:2` names. Its cost is not just the pass: it is that
every intermediate value passes through memory in the IR the *inliner* and the
early simplification passes see, so the decisions those passes take are taken on
a worse program.

### 2.3 The one place `alloca` is emitted, and why it is not a violation

Escape analysis (MEMORY.md §5.2) promotes a struct that is constructed and
consumed within one function, never stored into a heap value and never returned,
into stack memory. That is an `alloca` in the entry block.

It is not the case `LLVM-tips.md:2` is about. The instruction is about using
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
(`generate.rs:809-840`, `retarget_assigns`).

In block-argument SSA it is not assignment. The loop header block takes the
parameters as block parameters, and "rebind and continue" is a `Jump` back to the
header with the new values as arguments — which is a phi, in the right place,
with no slot involved. The same holds for the merged dispatch function of a
mutually-recursive group (`tail_calls.rs:47-51`), whose dispatch variable is one
more block parameter carrying the member index.

So the construct that would have forced mutable-slot emulation is the construct
that most naturally produces phis, and there is nothing to emulate. That
falls out of `middle::tail_calls` doing the rewrite rather than the backend doing
the emission (ARCHITECTURE.md §2.2), which was the reason to move it.

## 3. Attributes

`LLVM-tips.md:3`. The effect system supplies most of this for free, and it is
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
| No `ctx`, no effect-carrying `self`, reads no heap through a parameter | `memory(none)` |
| Same, but reads through a parameter (every `[T]`, `Str`, struct pointer) | `memory(argmem: read)` |
| Same, and can abort | add `inaccessiblemem: write` |
| Bounded only by `Alloc` — deterministic (SPEC 10.5) | `memory(argmem: read, inaccessiblemem: readwrite)` — the allocator is inaccessible memory |
| Bounded by an observable effect | no memory attribute; the default `memory(readwrite)` |
| Every function, on every backend | `nounwind` — there is no unwinding in the language at all (SPEC 6.10) |
| A function `middle` proved terminates | `willreturn`, and `mustprogress` |
| `memory(none)` + `willreturn` + `nounwind` | add `speculatable` — a call may be hoisted above a branch |
| Every function that cannot abort | `nofree` is *not* set: `decref` frees |

`nounwind` on every function is the single most valuable one and it costs no
analysis: the language has no `throw`, no `panic` that unwinds, and no
`catch`. An abort is `write` then `_exit` (`generate.rs:326-334` is the
JavaScript spelling of the same contract). LLVM without `nounwind` has to assume
every call is a potential unwind edge, which pessimizes everything downstream of
it.

### 3.2 What comes from the value model

| Buri property | LLVM attribute |
|---|---|
| Every pointer parameter that is not a niche-encoded `Option` (VALUE-MODEL.md §6) | `nonnull` on the parameter |
| Every heap pointer (16-byte aligned header, VALUE-MODEL.md §2) | `align 16` on the parameter and the return |
| Every pointer parameter, always | `readonly` — values are immutable, and no function in the language writes through a parameter |
| A parameter whose lifetime the caller guarantees for the call (MEMORY.md §5.2's *borrowed*) | `nocapture` |
| A pointer parameter the callee does not alias with any other parameter | `noalias` — §3.3 |
| Every `Str`/`[T]`/heap pointer return | `noalias` on the return, plus `align 16` |
| A parameter the layout says is `Bool` | `range(i8 0, 2)` |
| A `Char` parameter | `range(i32 0, 0x110000)` |

`readonly` on a parameter is the one that is true unconditionally and would be a
lie in almost any other language. Values are immutable, there is no interior
mutability, and `middle::rc`'s in-place reuse (MEMORY.md §5.3) is the only writer
— and it writes only to a block whose reference count it has just observed to be
1, which is to say a block no other reference reaches. So even reuse does not
break the guarantee the attribute makes to a *caller*.

Note that `readnone`/`readonly` remain valid **parameter** attributes in current
LLVM; it is the *function*-level spellings that were replaced by `memory(...)`.
§3.5 has the encoding hazard.

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

VALUE-MODEL.md §11.1 amends SPEC 6.2 to describe two backends whose overflow
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
an index that `list.get` bounds-checked into an `Option` (`list.buri:24-27`:
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
function (`tail_calls.rs:9-16`). Both are exact — "the emitted loop is what a
hand-written loop would have been" (`tail_calls.rs:18`) — and both reach LLVM as
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
because `tail_callees` collects only `ExprKind::CallFn` (`tail_calls.rs:82`). It
is a middle-end gap that predates this design, and the fix is a middle-end one.

## 6. Cache locality and basic-block size

`LLVM-tips.md:4`.

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
  arm chain (`generate.rs:1429-1470`), where a six-variant enum performs six
  comparisons to reach its sixth arm — `generate.rs:1453-1458` says so.
- **The unit is a source module** (ARCHITECTURE.md §5.1), so functions that call
  each other land in one `.text` section next to each other. Within a unit,
  emission order is the middle end's function order, which is the
  monomorphization worklist's — deterministic, and derived from the reachability
  walk out of the entry point rather than from a hash order
  (`monomorphize.rs:247-248`). That is a free first approximation of a
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
`typed::Expr::span`, which every node already carries (`typed.rs:29-33`), and
`DISubprogram`s from `Func::debug_name` (`monomorphize.rs:316`).

Two constraints from ARCHITECTURE.md §7: `DW_AT_comp_dir` is the repository root
spelled relatively, and on Mach-O the `N_OSO` entries name repository-relative
object paths. Both are set from `Options::unit_prefix` rather than from
`std::env::current_dir`, because `--check-reproducible` builds into two different
directories precisely to catch the version that does not.

## 8. Version pinning

```toml
[dependencies]
inkwell = { version = "0.10", optional = true, features = ["llvm21-1"] }
```

**LLVM 21.1, inkwell 0.10, `llvm-sys` 211.x, `LLVM_SYS_211_PREFIX`.**

inkwell supports LLVM 12 through 22 (`llvm22-1` is the newest flag) and currently
has zero lag behind stable LLVM. LLVM 22 would be the newest valid pin. 21 is
chosen instead because availability decides it:

- `nixpkgs` on the pinned `nixos-25.05` provides `llvmPackages_18` (18.1.8),
  `_19` (19.1.7, the default), `_20` (20.1.8) and `_21` (21.1.2). **There is no
  LLVM 22 on 25.05.** Pinning 22 would mean bumping the flake's nixpkgs before
  the backend can be built at all, which is a change to how the whole toolchain
  is built in service of a codegen decision.
- LLVM 21 is nixpkgs' *default* `llvmPackages` on 25.11, 26.05 and unstable, so a
  contributor on any of those gets it without naming a version.
- Homebrew ships `llvm@21` (21.1.8) for macOS contributors who are not on nix.

What 22 would buy is `musttail` on RISC-V, which §5 declines to use on any
target. The pin is reviewed when the flake's nixpkgs is next bumped, and moving
it is a one-line feature change plus a `Backend::identity()` bump that
invalidates cached release objects — which is correct, and is why `identity()`
exists (ARCHITECTURE.md §3).

**`strict-versioning` is on.** `llvm-sys` will otherwise build against any LLVM
at least as new as its target, so a machine with LLVM 22 installed would silently
produce a toolchain that generates different code from one built against 21.
For a compiler whose central claim is byte-identical output, "at least as new" is
not a version policy.

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
