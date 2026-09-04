# Native backends: architecture

Two native backends, one middle end, and a link step that reuses object files.
LLVM through `inkwell` for `--release`; the copy-and-patch backend of
CODEGEN-STENCIL.md, written here and depending on nothing, for everything
else. This document says where the code goes, what the interface between the
middle end and a backend is, how the build graph grows, and what
`--check-reproducible` means once an artifact is an executable.

Everything here is decided. Where a decision contradicts something already
written down — in `design/TODO.md`, or in the code — the contradiction is named
with a file and a line.

## 1. The shape of the problem

**Program-level decisions do not belong in an emitter.** Tail-call strategy,
the shape a `match` compiles to, and closure conversion are properties of the
program rather than of the language it is printed in: a linear arm chain is
wrong for every backend, closure conversion is required by two of the three,
and a tail-call plan two emitters each re-derive is two implementations of one
rule.

All three used to live inside the JavaScript emitter, because there was one
emitter and one file to put them in. Taking them out is what the middle end
(§2) is, and the `Backend` interface (§3) falls out of that rather than being
designed in front of it.

## 2. Module layout

```
cli/src/compiler/
  semantics/            unchanged by any of this
  middle/               the middle end
    mod.rs              the pipeline, and `strongly_connected`
    monomorphize.rs     the call graph, made exact
    inline.rs           inlining and folding
    dce.rs              reachability + drop, after inlining
    tail_calls.rs       *rewrites* rather than advises
    decision.rs         match arms -> a decision tree
    derives.rs          a generated Show/Eq/Hash/Json per type
    fuse.rs             combinator chains -> one loop, native only
    closures.rs         lambda -> code pointer + environment record
    rc.rs               own/borrow inference, elision, reuse
    layout.rs           the value model, as a computed table
    ir.rs               block-argument SSA, native only
    lower.rs            tree -> ir
  backend/
    mod.rs              the `Backend` and `Linker` traits, and `Emitted`
    runtime_native.rs   the embedded `libburi_rt.a`, and its hash
    js/                 generate.rs  javascript.rs  intrinsics.rs  runtime.js
    llvm/               mod.rs  emit.rs  repr.rs  attrs.rs  runtime.rs  target.rs
    stencil/            the copy-and-patch backend, eighteen files
                        (CODEGEN-STENCIL.md); on by default, and what `select`
                        returns for every native debug build (§4)
cli/src/build/
  link.rs               the incremental link
cli/src/commands/
  watch.rs              the poll loop (see BUILD-AND-WATCH.md)
cli/runtime/            the native runtime, Rust, C ABI
  lib.rs                the `buri_rt_*` ABI contract, which both backends cite
  memory.rs abort.rs value.rs host.rs http.rs rng.rs list.rs text.rs fmt.rs …
cli/build.rs            builds `cli/runtime` into `libburi_rt.a` for the host
```

`transform` becomes `middle` because the name has to say what the thing is. A
`transform` module is a place passes go; a middle end is the half of a compiler
between the front end and code generation, and after this change that is exactly
what it holds — including the value model, which is not a transformation of
anything.

### 2.1 Two layers, and why the middle end is not one IR

The middle end has two layers, and the split is load-bearing.

**Layer A — the tree.** `monomorphize` through `closures`, operating on
`typed::Expr` bodies. Every backend consumes this. It is where inlining, folding,
dead-code elimination, tail-call rewriting, decision trees and closure conversion
happen, and it is whole-program: monomorphization makes the call graph exact
(`monomorphize.rs`'s header), so every decision taken here is a fact rather than
an estimate — which is already the argument the inliner makes for sitting where
it does.

**Layer B — the CFG.** `middle::ir`, a per-function control-flow graph of basic
blocks with **block parameters**, produced by `middle::lower` from the layer-A
tree. Only the native backends consume it.

The JavaScript backend does not get a CFG, and that is the decision rather than
an omission. Going from a CFG back to structured JavaScript needs a relooper, and
a relooper would take a backend that today prints code a human can read
(`Profile::pretty` — debug builds stay readable, because the names are what
make a stack trace useful) and make it print a state machine. The gain would be
zero: everything JavaScript needs from the shared work is in layer A.

The counter-proposal — no layer B, and each native backend builds its own SSA —
is rejected on CODEGEN-LLVM.md §0's second instruction. "Avoid `mem2reg`,
generate optimized SSA form" is one the frontend can only honour by having SSA
*before* it reaches LLVM. LLVM will not build it, and having one backend's SSA
be real and the other's be an artifact of `alloca` is exactly the divergence that
makes two backends disagree. Build it once, in the middle, in the block-argument
form: a block parameter is a frame slot the predecessor writes in the
copy-and-patch backend (CODEGEN-STENCIL.md §0) and a mechanical transliteration
into LLVM phis (see CODEGEN-LLVM.md §2). The form was chosen when the debug
backend was Cranelift and CLIF was block-argument SSA itself; it outlived that
reason rather than depending on it.

### 2.2 What each new layer-A pass is for

- **`dce.rs`.** CODEGEN-LLVM.md §0's first instruction: dead code elimination
  before it reaches LLVM IR. Monomorphization already gives reachability-based DCE for free
  (`monomorphize.rs`'s header), but inlining creates *new* dead functions — a
  body inlined at its single call site leaves the original unreachable, which
  the inliner said out loud and then left for `javascript::eliminate_dead`
  to drop by name. Dropping by name is a JavaScript minifier's job. A native
  backend needs it dropped by index, before layout and codegen spend time on it.
  So DCE moves into the middle end and the minifier stops being where it happens.
- **`tail_calls.rs` rewrites.** Today it produces a `Plan` the emitter consults
  (`tail_calls::Plan`), and the emitter has to agree with it about what tail
  position is — `tail_calls.rs`'s header records that a disagreement produces a
  `while (true)` nothing ever continues, "which looks like elimination and is
  not". Two implementations of one rule is one too many at one backend and three
  too many at three. It becomes a rewrite: a self-looping function gets an
  explicit `Loop`/`Continue` node in its body, a merged group becomes one
  function with a dispatch parameter, and every backend emits what it is given.
  The `Plan` type stays as the analysis; the emitters stop reading it.
- **`decision.rs`.** `js::generate`'s `arm_chain` tests arms in order, which
  is O(arms) comparisons to reach the last one — its own comment says so
  and offers a release-mode shortcut that only helps the final arm. A decision
  tree over the scrutinee's discriminants is O(1) for an enum match and is the
  shape a `switch` wants in JavaScript and in LLVM, and a tree of tests wants in
  an emitter with no instruction selection at all. One pass, three
  beneficiaries.
- **`closures.rs`.** `ExprKind::Lambda { captures }` already carries the capture
  list. Conversion turns a lambda into a top-level function taking an environment
  as its first parameter plus a construction of that environment. It is sound
  without analysis because SPEC 10.6 forbids capturing an effect-carrying value,
  so an environment is always plain immutable data.
- **`layout.rs`.** The value model of VALUE-MODEL.md, computed once per type and
  memoised. It is in the middle end rather than in a backend because both native
  backends must agree byte for byte — an `[T]` whose element stride the debug and
  release backends disagree about is a miscompile that only shows up between
  profiles.

### 2.3 What the JavaScript backend loses and gains

Loses: the `Plan` consultation, `arm_chain`, and dead-code elimination by name.
Gains: decision trees, and closure conversion it will immediately undo.

Closure conversion is a pessimisation in JavaScript — an arrow function closing
over its scope is what the engine wants. So the JS backend is handed the tree
**before** `closures` runs, and the native backends after it. The pipeline is
therefore a sequence with one branch at the end, not a straight line:

```
monomorphize -> inline -> dce -> tail_calls -> decision
                                                  |
                +---------------------------------+
                |                                 |
               js            derives -> fuse -> closures -> rc -> layout -> lower -> ir
                                                                                     |
                                                                   +-----------------+-------------+
                                                                   |                               |
                                                               stencil                            llvm
```

Folding is not a stage of its own: it is inside `inline`, interleaved with it,
because inlining a constructor into a projection is what makes most folding
possible and folding is what exposes the next round's call sites. That is why
`optimize.rs` becomes `inline.rs` rather than `inline.rs` plus `fold.rs`.

`derives`, `fuse` and `rc` are on the native branch only. `derives` generates a
`Show`, `Eq`, `Hash` and `ToJson`/`FromJson` per type where JavaScript walks a
descriptor at run time (VALUE-MODEL.md §9); `fuse` collapses a combinator chain
into one loop, deleting an intermediate list whose cost is `malloc` plus a copy
natively and a bump pointer in a nursery on JavaScript; `rc` inserts and elides
reference-count operations, which a garbage-collected target has no use for
(MEMORY.md §5.2). `fuse` runs after `derives`, so that a generated body's own
chains fuse, and before `closures`, because fusion composes the lambdas that
`closures` is about to lift. Leaving JavaScript unfused also leaves it as the
reference the agreement tests compare both native backends against.

`middle::run` is layer A, and `middle::native` is the branch after it; each
backend asks for what it needs and there is no "the pipeline" constant that
three callers copy. Both mutate the program in place rather than returning a new
one, because function indices never move — `inline.rs`'s own invariant, which a
pipeline returning a fresh `Program` would make harder to keep than to state.

**Amended in wave 3c: a backend cannot ask, so the build system composes.**
`middle::native` needs the program by `&mut` and `Backend::emit` is handed it by
`&` — which is the type saying that a backend transforms nothing — so "each
backend asks for what it needs" was not implementable as written. The
composition is one function, `actions::prepare(program, target)`: layer A
always, the native branch when the target is not `Js`. Both callers that reach a
backend go through it (`actions::emit` for a text artifact,
`actions::objects_of` for objects), so there is one place that decides which
passes a target gets, and `backend::select` is the only other thing the two
paths share. A third caller would be the bug this shape exists to make visible.

The cost of putting the seam there is that `middle::lower` runs twice on a
native build: once in `objects_of`, whose `codegen` keys are hashes of the
lowered IR, and once inside `Backend::emit`, which lowers for the bytes.
Lowering is deterministic and a pure function of the program, so the two agree
by construction, and the alternative — `emit_lowered` on the trait, taking an
`ir::Program` — buys one lowering at the price of a second entry point that the
JavaScript backend cannot implement and that every future caller could choose
instead of the seam. Measure before revisiting: at the sizes the conformance
corpus reaches, lowering is a small fraction of a native build.

## 3. The `Backend` trait

The signature this was drawn from was:

```rust
trait Backend { fn emit(&Program, &Tables, &Options) -> Result<Vec<u8>, Diagnostics> }
```

That signature is amended, for one reason: `Vec<u8>` is one artifact, and the
whole of the incremental-link plan is that a build emits *many* object files and
relinks only the ones that moved. A trait that can only return one blob makes the
feature unrepresentable, and the shape of it would have to be smuggled through
`Options` or through the filesystem.

```rust
/// One codegen unit's output.
///
/// `key` is the cache key the unit was produced under, and it is computed by the
/// backend rather than by the build system: only the backend knows which of its
/// own inputs — target triple, LLVM version, pass pipeline — the bytes depend on.
pub struct Emitted {
    /// Stable, deterministic, and a filename: `lib_money.o`, `main.mjs`.
    pub name: String,
    pub key: ActionKey,
    pub bytes: Vec<u8>,
}

/// Platform and architecture together, because a backend needs both and the
/// build system already carries them as a pair on every `Output`.
pub struct Target {
    pub platform: Platform,
    pub arch: Option<Arch>,
}

pub struct Options<'a> {
    pub profile: Profile,
    pub target: Target,
    /// Repository-relative, for the paths a debug section records.
    pub unit_prefix: &'a str,
}

/// Which codegen units an emission is for. The build system keys one cache
/// entry per unit (§6.2) and serves every hit from the cache, so the units it
/// still needs after a one-line edit are usually one of several hundred.
pub enum Units<'a> {
    All,
    /// Unit indices into `ir::Program::units`, which is the order
    /// `Backend::emit` returns its objects in.
    Only(&'a [u32]),
}

pub trait Backend {
    /// `js`, `stencil`, `llvm`. Enters every cache key this backend produces.
    fn name(&self) -> &'static str;

    /// The identity of everything outside the program that the bytes depend on:
    /// the LLVM version, the stencil library's digest, the runtime's own hash.
    /// Enters every cache key. A backend that returns a constant here is claiming its
    /// output cannot change without the toolchain hash changing, which is true
    /// of `js` and of nothing else.
    fn identity(&self) -> String;

    /// Intrinsic keys this backend has no implementation of, so
    /// "missing intrinsic" becomes a question asked per backend
    /// (`design/TODO.md`, "The native backend").
    ///
    /// Takes `&Tables` as well as the program: deciding whether a key has a
    /// body goes through the same code the emission does — `Gen::intrinsic`,
    /// which resolves `num.*` through the *type* of the function it is
    /// implementing — and that needs the type table. A version that did not
    /// take it would be a second implementation of the question, and the two
    /// would drift, which is the failure this signature exists to prevent.
    fn missing_intrinsics(&self, program: &Program, tables: &Tables) -> Vec<String>;

    fn emit(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics>;

    /// `emit`, restricted to the units the caller still needs. The default
    /// emits everything and is correct rather than fast, because a superset
    /// satisfies every caller: the build system takes the objects it asked for
    /// by name and serves the rest from the cache.
    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics>;
}

pub trait Linker {
    /// Enters the `link` key, with `version()`. Both are named here because
    /// §6.2's `link_key` reads them, and a trait that could not answer them
    /// would make that key unbuildable.
    fn name(&self) -> &'static str;
    fn version(&self) -> String;

    /// Combines units into the final artifact at `out`. `unchanged` names the
    /// units whose bytes are byte-identical to the previous link, which a linker
    /// may use and may ignore.
    fn link(
        &self,
        units: &[Emitted],
        unchanged: &[usize],
        out: &Path,
        opts: &LinkOptions<'_>,
    ) -> Result<(), Diagnostics>;
}
```

Five things about this signature are decisions:

- **`&mut self` on `emit`.** An LLVM `Context` is not `Sync` and owns everything
  built inside it; a `&self` signature would force interior mutability on the one
  backend that most wants a plain owned object.
- **`Vec<Emitted>` even for JavaScript.** The JS backend returns exactly one
  element and its `Linker` is "take element zero". A special case for the backend
  that has one unit would be a second code path through the build system for the
  only backend currently covered by tests.
- **`identity()` separate from `name()`.** `Profile::Release` on LLVM 20 and
  `Profile::Release` on LLVM 21 must not share a cache entry, and the toolchain
  hash does not catch it: `llvm-sys` links against whatever `llvm-config` found
  at build time, so two `buri` binaries with identical Rust source can have
  different LLVM underneath. `identity()` is where that gets into the key, and
  it is the backend's own answer because the build system has no way to ask.
- **`missing_intrinsics` takes the program, not a list of strings.**
  `check_intrinsics` today takes a `&[String]` the emitter accumulated as a side
  effect of emission (`js::generate`), so a program can
  only be told what it is missing *after* a failed emission. Asking the backend
  up front means `buri build --output=linux/arm64` on a program using an
  unimplemented intrinsic reports it before spending a second in LLVM.
- **`emit_units` alongside `emit`, with a default.** The per-unit parameter was
  the one this interface was missing, and its absence was measurable: at 118k
  lines a one-line edit cost 2,622 ms, of which 64% was `emit` re-producing
  several hundred already-cached objects that were then thrown away
  (`design/PERFORMANCE.md` §6.5). The default implementation forwards to `emit`,
  so a backend with one unit — JavaScript, whose `Linker` takes element zero —
  implements nothing, and a backend that gains a per-unit path is a change to
  one file rather than to the interface.

`Profile` stays two-valued and grows nothing. It moves out of `js::generate` and
into `backend/mod.rs`, because `Profile::defensive_aborts` is a statement about
programs and not about JavaScript.

## 4. Backend selection

```
(Js | Web,       _)        -> js
(Linux | Macos,  Debug)    -> stencil
(Linux | Macos,  Release)  -> llvm
```

`Web` joins `Js` because the question this match asks is "which backend emits
this artifact", and a page is JavaScript.

**There is no third native backend, and there was until 2026-08-29.** The debug
row read `cranelift` for as long as `stencil` was compiled in and never
returned — the arrangement that kept taking the seat a decision about parity
rather than about plumbing. Parity was met, the seat was taken, and the
retargetable backend was removed from the tree with its design document;
CODEGEN-STENCIL.md §13 is the reversal, its reasons and its costs, and
DECISIONS.md's three rows point at it.

The debug row is not every native triple. `stencil` carries one stencil library
per (instruction set, container) pair and refuses a target it has no library
for, so `select` returning it is necessary and not sufficient:
`actions::native_ready` still asks the backend. The libraries are `macos-arm64`,
`linux-arm64` and `linux-x86_64` (CODEGEN-STENCIL.md §3.2).

**macOS on x86-64 has no library, and it stays that way.** It is the one
native triple with no debug backend at all, and that is an honest unsupported
target rather than a hole: `stencil::supported` refuses it by name — *"the
stencil backend has no stencil library for macos-x86_64"* — so `native_ready`
is false there, `driver::host_platform()` answers `Js`, and `buri build`
refuses with a sentence naming the target. That is the same shape a host with
no `cc` gets. CODEGEN-STENCIL.md §3.2 and §9 carry the argument.

The published measurements the split was weighed against — a retargetable
generator's standing between LLVM `-O0` and a template JIT — are Xu and
Kjolstad's *Copy-and-Patch Compilation* and Schwarz, Kamm and Engelke's *TPDE: A
Fast Adaptable Compiler Back-End Framework*, both linked from
[../../reference/README.md](../../reference/README.md). They are also why the
debug row could change hands on a measurement rather than on an opinion.

Two things sit on top of the table:

- **There is no `--backend` flag, and the agreement test does not need one.**
  Selection is `backend::select(target, profile)` and it takes no name, so a
  backend is chosen by what is being built rather than by an argument a user
  passes. The cross-backend differential test — the native analogue of the existing
  `release_and_debug_agree` — is `cli/tests/native/agreement.rs`, which compiles
  one source twice from one analysis, through `actions::prepare` and `select`
  for each side, and compares stdout byte for byte. It is written against
  VALUE-MODEL.md §12's divergence table, one `#[test]` per row, so a failure
  names the row (VALUE-MODEL.md §12).
- A build of the toolchain without the `backend-llvm` feature refuses a native
  release build with a diagnostic naming the feature, rather than silently
  falling back to the debug backend. Falling back would mean `--release` produced
  different code depending on how the compiler was installed, which is the same
  class of bug as an unpinned toolchain. See BUILD-AND-WATCH.md §2.

`driver::host_platform()` is one line and a condition:
`native_ready(host, Debug)` — the host's own platform where this toolchain can
produce something for it (a backend compiled in, a runtime archive, a linker),
and `Js` where it cannot. That keeps a toolchain built `--no-default-features`,
and any host that is not macOS or Linux, answering `Js`.

**It is not what decides the artifact**, which is why flipping it cost no
golden-file churn: `Output` decides, and the build system reads the *declared*
outputs. `host_platform()` reaches the language server and the documentation
harness, where it sets a compilation's platform and where nothing varies on it.
What *does* vary on the host is `buri run`, which prefers a declared
host-native output and executes the artifact directly, and `buri test`, which
runs a suite as a native binary whether or not it named one.

**`buri test`'s default has flipped; `selected_outputs`' has not.** A suite that
names no platforms runs **natively**, in the dev profile, on the host it is
already checked against. A binary that declares no outputs still gets `JS`.

The two halves moved apart because the argument for waiting was never about the
backend, it was about the refusal: `Backend::missing_intrinsics` refuses a
program reaching something the backend has no body for, which is the right
refusal and the wrong default — a `buri run` that fails on a program `buri run`
used to run is not an improvement. A *test* can have it both ways, and now does.
`run_suite` asks the same hook before it commits, and a suite whose program
names a gap runs on JavaScript with a line on stderr saying which gap
(`commands/test.rs`'s `native_gap`). So the refusal is still there for anyone
who asked for a native run by name, and nobody who did not ask is refused
anything. The measured reason for spending the fallback is
`design/PERFORMANCE.md` §6: the native dev loop is now the faster one on both
halves of a 104k-line edit-test cycle.

`selected_outputs` has no such escape. A binary that declares no outputs is
asked for an *artifact*, and an artifact that silently changed platform would
change what `buri run` executes and what a release ships. That flip stays what
it was — one line, when the refusal goes quiet across the conformance corpus.

`actions.rs` and `commands/build.rs` and `commands/test.rs` each refused a non-JS
platform with "the backend is not implemented", and all three are gated on
`native_ready` instead, in the wave that owned each (2c, 3a, 3c). **The wording
was kept and should not have been.** `native_ready` is a conjunction of three
questions and it answered a `bool`, so the one sentence had to cover all three —
and it covered two of them falsely. A `linux/x86_64` output on a mac is refused
by the *host*, on a machine that had just built `macos/arm64` from the same rule;
a `--release` build without `backend-llvm` is refused by the *profile*, on a
toolchain whose debug build of the same output works. Both were told "the
{platform} backend is not implemented; this toolchain emits JavaScript, build
with `--output=js`", which named the wrong thing and then pointed at a fix that
was not one (buri-lang/buri#25, buri-lang/buri#26).

`build/actions.rs`'s **`native_gap`** is the repair: the same three questions in
the same order, answering *which* one failed as an output, a reason and a fix, and
`native_ready` is now "is there no gap". All three sites print it through one
templated diagnostic, `native-artifact-not-available`, so they cannot describe
one gap three ways. `repositories/cli/output_selection` pins the host half and
`backend::select`'s own rows pin the profile half — the profile half cannot be a
golden, because what `--release` answers for the host's own target depends on
which leg of `cli/tests/README.md`'s bar the toolchain was built on.

What did **not** change is the release refusal itself. A toolchain without
`backend-llvm` still refuses `--release` rather than falling back to the
development backend, for the reason §3 gives above: `--release` producing
different code depending on how the compiler was installed is an unpinned
toolchain by another name. What the reader gets now is the true reason and the
two things that would fix it — build without `--release`, which the development
backend has the target for, or install a toolchain built with the feature.

**One thing those two cases now depend on that they did not:** the *host*. On a
Linux x86_64 machine `--output=linux/x86_64` is no longer refused — it builds —
so both fixtures pass on macOS and on a host whose platform they do not name, and
fail on the host they do. Fixing that needs the harness to know which platform is
the host and which is the cross one, since the refusal names a platform and the
golden is text. It is the one piece of golden work wave 3c did not do.

## 5. Codegen units

### 5.1 One unit per source module

A codegen unit is **the set of monomorphized functions whose declaration came
from one source module**. `Func::debug_name` is already `module:owner.name`
(`monomorphize.rs`), so the partition exists in the data; it becomes an
explicit `Func::unit: u32` assigned by `middle::lower`.

Three candidates were considered.

- **One unit per function.** Zig's self-hosted linker works this way, and it
  gives the finest possible incrementality. Rejected on link cost: a
  conformance-sized program monomorphizes into thousands of functions, and a
  thousand-member archive is slower to link than the compile it saved. It also
  makes every intra-module call go through the linker, which costs a relocation
  where a direct branch would do.
- **A fixed count, merged.** What rustc does — sixteen CGUs, partitioned by
  module and merged down. Rejected because the merge is the part that hurts: two
  unrelated modules in one unit means an edit to either invalidates both, and the
  count is tuned for rustc's parallelism rather than for reuse.
- **One per source module.** Chosen. It is the unit an edit is scoped to, which
  is the only property that matters for reuse, and it is a partition a developer
  can predict without reading the compiler.

The standard library is one unit per standard-library module on the same rule, so
a program that touches two functions of `core/list` pays for one `core/list`
object and reuses it across every build **of that target**. That is a large,
free win: the standard library is thirty modules that essentially never change.

The reuse stops at the target rather than at the repository, and always did.
Monomorphization makes a unit's IR a function of the whole program it is in, so
two binaries' `core/list` objects are the same bytes only where neither
instantiated anything the other did not. Measured on a 118k-line repository with
two native binaries over one library: **2 of 369 codegen units** were shared
across the pair, and the cold `buri build //...` cell does not move when they
are not (1.46 s against 1.49, one run each, inside the noise). That is why
`unit_prefix` being a term of the `codegen` key (§6.2) costs so little: it ends
the cross-target sharing, and there was almost none to end. A batched test
binary spans packages under one empty prefix and shares within itself, which is
where the sharing that matters happens.

A unit over a node budget (default 40 000 IR nodes) is split at function
boundaries into `foo.0`, `foo.1`, ..., deterministically by the existing function
order — which is source order, which is what `monomorphize.rs` already
guarantees for reproducibility.

### 5.2 The same partition in both profiles

Release does not merge units and does not use LTO. The reasoning:

Cross-unit inlining is the thing LTO exists to recover, and in this compiler
inlining has already happened — in the middle end, over an *exact* call graph,
with no dynamic dispatch anywhere in the language to blunt it
(`monomorphize.rs`). LTO would be re-deriving a worse version of a decision
already taken with better information. What LLVM contributes at release is
machine-level: instruction selection, scheduling, register allocation, vectorization,
and the peepholes — all of which are function-scoped or unit-scoped and lose
nothing to a unit boundary.

What is lost is inlining *into* the runtime's own functions, since `cli/runtime`
is a prebuilt archive. That is real, and it is bounded: the runtime's hot entries
(`incref`, `decref`, `alloc`) are not called, they are open-coded by the backends
(MEMORY.md §5), so the archive contains only operations that are large enough
that a call is noise.

Release does two things Debug does not: every function not reachable from a root
is given internal linkage, so LLVM may specialize and delete it; and the middle
end runs with a larger inline budget (`inline::Options::rounds` goes from 3 to
6, `SINGLE_USE` from 96 to 256).

If measurement later says ThinLTO is worth it, it is additive — a per-unit bitcode
emission and a second link step — and nothing here forecloses it.

## 6. The action graph

### 6.1 New actions

`cache::Action` gains one variant:

```rust
pub enum Action { Proto, Compile, Codegen, Link, Test }
```

`Codegen` is one action per codegen unit. `Compile` stays what it is — the
front-end key that `--explain` reports and nothing stores.
`Link` stays the artifact key.

Per profile the graph is the same shape; only the backend differs:

```
proto?   ->  compile (per closure member)  ->  codegen (per unit)  ->  link
```

### 6.2 Keys

`Codegen`'s key is **content-addressed on the IR**, not on source files:

```
codegen_key(unit) = H(Codegen, toolchain, mode, platform, arch,
                      backend.name(), backend.identity(),
                      unit_prefix,
                      H(the unit's lowered IR),
                      H(the layout of every type the unit names))
```

This is the decision the whole incremental story rests on, so it is worth being
explicit about why it is not the obvious thing. The obvious thing is to key a
unit on the sources of the module it came from, the way `actions::contribute`
keys a target. That is wrong here in both directions.
It is *unsound*, because a monomorphized unit contains instantiations requested
by other modules — `core/list`'s object for a program depends on which types that
program maps over — and it is *imprecise*, because reformatting a comment in
`parse.buri` changes its bytes and not one instruction of its IR.

Hashing the IR fixes both. The IR is what codegen reads, so hashing it is hashing
the input; and it is insensitive to everything that is not semantics, so a
whitespace edit produces an identical key and the object is reused.

The IR is not *all* codegen reads, and the rest of `Options` is in the key for
the same reason: `profile`, `target`, and `unit_prefix`. The prefix is there
because §7 makes it reach the object — the paths a debug section records are set
from it — and because it already does on every ELF target, where LLVM emits a
unit's module name as a `.file` directive and therefore as an `STT_FILE` symbol.
It costs the cross-package reuse §5.1 counts on: two targets whose closures share
a unit compile it twice, because they are two prefixes. A key that omits an
input to codegen is a key that can serve bytes codegen would not have produced,
and that is the one thing this key exists to rule out.

The cost is that computing the key requires running the front end and the whole
middle end, so `codegen` can never be skipped without doing the analysis. That is
acceptable and, in this compiler, nearly free: `conformance build //...` measures
22 ms end to end, and the expensive half of a native
build is the half the key is protecting.

`Link`'s key is the ordered list of `codegen` keys plus the linker's identity:

```
link_key = H(Link, toolchain, mode, platform, arch,
             linker.name(), linker.version(),
             [codegen_key(u) for u in units],   // ordered
             runtime_archive_hash | "omitted")
```

Ordered, because link order determines symbol resolution order and therefore
determines the bytes. The last term is the archive's **decision** rather than
its digest: since 2026-08-30 the link names `libburi_rt.a` only when the objects
carry a `buri_rt_*` symbol (BUILD-AND-WATCH.md §2.2), and a link that does not
name it does not depend on it. Two decisions are two command lines and therefore
two keys; one term either way, because an omitted archive has no digest to
state. Both keys are built with the existing `KeyBuilder`, which
already length-prefixes every field (`cache.rs`, `Sha256::field`) so two different
field decompositions cannot collide.

### 6.3 Where objects live

```
.buri/cache/<ab>/<rest>        the object bytes, content-addressed, as today
.buri/out/<platform>-<arch>/<pkg>/<artifact>          the executable
.buri/link/<link-key>/manifest                        unit name -> codegen key
.buri/link/<link-key>/<unit>.o                        hard link or copy from cache
```

The `.buri/link/<key>/` directory exists because a linker takes paths, not bytes,
and because the manifest is what makes "which objects changed" answerable without
re-running codegen. `Cache::get` already returns bytes; the link step writes them
into the link directory under stable filenames, hard-linking where the filesystem
allows it so a large object is not copied twice.

The manifest is the input to CODEGEN-STENCIL.md §12.2's incremental relink, and
it is also what `--explain` reads to print one `codegen` line per unit with its
status. `buri clean` takes `.buri/link` with the rest.

`actions::artifact_path` already produces
`.buri/out/<output.dir()>/<pkg.path>/<name>` and `Output::dir()` already
produces `linux-x86_64`. The only change is that
`Platform::Js => format!("{base}.mjs")` gains no sibling: a native artifact's
name is `base`, with no extension, which is what `artifact_path`'s `_` arm
already does.

## 7. `--check-reproducible` for a native artifact

`commands::build::check_reproducible` builds twice into two directories, from
two fresh sessions, with the cache off, and compares bytes; its own header
states why each of those three is load-bearing. It refuses non-JS platforms
today.

Three changes.

**It compares objects first, then the executable.** `actions::first_difference`
reports a byte offset, and a byte offset into a four
megabyte executable names nothing a person can act on. Compared per unit, the
report is "`core/list.o` differs, first at byte 4192" — which names a module, and
a module names a pass. The executable is compared too, because a reproducible set
of objects and an irreproducible link is a real failure mode (link order, archive
member ordering, a temporary path in a debug section) and it is the failure mode
a per-object comparison would hide.

**It runs codegen twice in one process rather than shelling out.**
`actions::compile_artifact` is already split out for exactly this. The native
equivalent stops after `Backend::emit` for the object comparison, then links
both sets into the two round directories.

**Three sources of nondeterminism are closed at the source rather than compared
for.** Each is a known one and each has a name:

- **Mach-O `LC_UUID`.** Emitted with `-no_uuid` (`ld64.lld`) / `-Wl,-no_uuid`.
  A content-derived UUID would be reproducible; a random one is not, and the flag
  removes the question.
- **Absolute paths in debug info.** The `DW_AT_comp_dir` and, on Mach-O, the
  `N_OSO` stab entries name the object's path on disk. Both are set from
  `Options::unit_prefix`, which is repository-relative — which is the same rule
  `actions::action_key` already follows for input paths ("paths are
  repository-relative, so two checkouts in different directories produce
  identical keys"). This is precisely the failure the two-directory design exists
  to catch, so it must be closed rather than tolerated.
- **Timestamps in archive members and in the Mach-O/ELF headers.** Zeroed.
  `SOURCE_DATE_EPOCH=0` is already in the action environment
  (`build/spawn.rs`; `buri docs build/hermeticity`) and the object writer honours
  it directly rather than
  through the environment, since the object writer is in-process.

The claim the flag then earns is unchanged in wording and stronger in content:
two builds of the same commit produce identical bytes, and now the bytes are an
executable.

## 8. Implementation waves

The waves this section scheduled have all landed. The schedule itself is not
kept: what it planned is now the module layout in §2, the trait in §3, and the
action graph in §6, and a plan for finished work is a second description of
those that nothing checks. What remains open is in `design/TODO.md`, under
"The native backend".

## 9. What this does not do

- **No cross-*linking*.** Cross *codegen* works and is exercised. The debug
  backend bakes one stencil library per target and looks one up by triple rather
  than by the running CPU, cross-building both of its Linux libraries on a macOS
  host with no Linux sysroot (CODEGEN-STENCIL.md §3.2), and LLVM targets
  everything: the benchmark suite takes `aarch64-apple-darwin`,
  `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` as default rows on
  whichever machine it is run on (`design/PERFORMANCE.md` §3). A cross triple is
  in fact *more* reproducible than the host one, because the host ISA is
  inferred from the running CPU's features and a cross ISA is the baseline for
  its triple.

  What is still refused is producing a runnable artifact for another host:
  `buri build --output=linux/x86_64` on a macOS host is an error naming the host
  it can build for. The policy lives in one predicate, `link::can_link` — the
  target's platform and architecture must be the host's — with
  `actions::native_ready` in front of it. The reason is the runtime archive (§2,
  `cli/runtime`), which `cli/build.rs` builds for the host and for nothing else,
  and which a cross link would need alongside a cross libc and a sysroot. So the
  fix, when someone wants it, is "ship prebuilt runtime archives per triple": a
  packaging problem rather than a compiler one. Saying that is the point of
  refusing out loud.
- **No dynamic linking, no shared libraries, no `dlopen`.** The language has no
  FFI to declare one with, so there is nothing to link against but the runtime.
  What "static" *means* is different on the two platforms, though, and the word
  on its own has been read as a promise the macOS artifact does not keep — so
  both are written out:

  - **Linux: a static-PIE executable against a musl the toolchain carries.**
    Not the machine's libc, and not a distribution's musl either. `cli/build.rs`
    builds `libburi_rt.a` for `<arch>-unknown-linux-musl` and bakes eleven files
    out of that rustc's own `self-contained/` directory — `libc.a`,
    `libunwind.a`, and the crt objects — into the `buri` binary;
    `build/link.rs` writes them back out beside the objects and points the
    driver at them. So the libc an artifact carries is a property of the
    toolchain that built it and not of the machine that ran the build, which is
    the whole point: a `buri build` on a 2024 distribution has to produce
    something that runs on a 2018 one. CODEGEN-STENCIL.md §12.3 has the command
    line and the three tiers.
  - **macOS: dynamically linked against libSystem, because there is no other
    option.** Apple ships no static libc, has not since 10.4, and a binary that
    bypassed `libSystem.dylib` to make raw syscalls would be one Apple has said
    it may break in any release. Every other dependency is still static — the
    runtime archive is in the artifact — so what "dynamic" costs here is one
    library that is present on every macOS by definition.

  Three consequences follow from the Linux half, and none of them is
  hypothetical:

  - **`dlopen` is not merely forbidden, it is absent.** The rule above is a
    design decision; a static-PIE musl binary makes it a fact of the file. That
    is also the reason a *statically linked glibc* was not the answer: glibc's
    `getaddrinfo` dlopens a matching `libnss_*.so` at run time, so a
    "statically linked" glibc artifact still needs a `libnss_files.so` of the
    right version to be present, and the thing the static link was for is
    exactly the thing it fails to deliver.
  - **Name resolution is musl's, which reads `/etc/resolv.conf` and
    `/etc/hosts` and nothing else.** There is no NSS, so `nsswitch.conf` is not
    read and mDNS, LDAP, NIS and `systemd-resolved`'s own plugin do not
    participate. This is not academic: `cli/runtime/http.rs`'s `resolve` calls
    `to_socket_addrs`, which is `getaddrinfo`, so a Buri program fetching a
    `.local` name — or a corporate name served only by an NSS module — will not
    find what a glibc program on the same machine finds. It is accepted rather
    than worked around because the alternative above does not work at all, and
    because the failure is a name that does not resolve rather than a binary
    that does not start.
  - **musl's `malloc` is slower than glibc's under multithreaded churn**, and
    the runtime allocates: `cli/runtime/lib.rs` §5 is `malloc`-backed, one block
    per allocation. What keeps that from being a per-value trip into a
    contended allocator is `cli/runtime/memory.rs`'s G2 per-thread block caches
    — a free returns the block to this thread's cache and the next allocation
    of that size takes it back without touching `malloc` at all (`rt.rs`'s
    thread-local note explains why a cached block may cross threads safely). If
    it ever does bite, **the answer is to bundle an allocator into the runtime
    archive, not to go back to glibc**: an allocator is a dependency this
    project can carry, and a libc the artifact does not carry is the property
    the whole section is about.
- **No threads.** This is not a native-backend decision, it is the language's,
  and MEMORY.md §3 records what it buys: non-atomic reference counting.
