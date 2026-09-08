# Native backends: architecture

Two native backends, one middle end, and a link step that reuses object files.
`--release` goes through LLVM via `inkwell`. Everything else goes through the
copy-and-patch backend of CODEGEN-STENCIL.md, which is written here and depends
on nothing.

This document says where the code lives, what the interface between the middle
end and a backend is, how the build graph grows, and what
`--check-reproducible` means once an artifact is an executable.

## 1. The shape of the problem

**Program-level decisions do not belong in an emitter.** Tail-call strategy, the
shape a `match` compiles to, and closure conversion are properties of the
program, not of the language it is printed in — and a tail-call plan two
emitters each re-derive is two implementations of one rule. Taking all three out
of the JavaScript emitter is what makes the middle end (§2), and the `Backend`
interface (§3) falls out of that rather than being designed in front of it.

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
    derives.rs          a generated Show/Equal/Hash/Json per type
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

### 2.1 Two layers, and why the middle end is not one IR

**Layer A — the tree.** `monomorphize` through `closures`, operating on
`typed::Expr` bodies. Every backend consumes it. It is whole-program:
monomorphization makes the call graph exact (`monomorphize.rs`), so every
decision taken here is a fact rather than an estimate.

**Layer B — the CFG.** `middle::ir`, a per-function control-flow graph of basic
blocks with **block parameters**, which `middle::lower` builds from the layer-A
tree. Only the native backends consume it.

The JavaScript backend gets no CFG on purpose. Going back to structured
JavaScript needs a relooper, which would take a backend that prints code a human
can read (`Profile::pretty`) and make it print a state machine. Everything
JavaScript needs from the shared work is in layer A.

Layer B is shared rather than built once per native backend because
CODEGEN-LLVM.md §0's second instruction — "avoid `mem2reg`, generate optimized
SSA form" — can only be honoured by having SSA *before* LLVM sees it, and one
backend's SSA being real while the other's is an artifact of `alloca` is exactly
the divergence that makes two backends disagree. So it is built once, in
block-argument form: a block parameter is a frame slot the predecessor writes in
the copy-and-patch backend (CODEGEN-STENCIL.md §0) and a mechanical
transliteration into LLVM phis (CODEGEN-LLVM.md §2).

### 2.2 What each new layer-A pass is for

- **`dce.rs`.** CODEGEN-LLVM.md §0's first instruction: eliminate dead code
  before it reaches LLVM IR. Monomorphization already gives reachability-based
  DCE for free, but inlining creates *new* dead functions — a body inlined at
  its single call site leaves the original unreachable, which
  `javascript::eliminate_dead` dropped by name. A native backend needs it
  dropped by index, before layout and codegen spend time on it.
- **`tail_calls.rs` rewrites** rather than producing a `Plan` an emitter has to
  agree with about what tail position is. A disagreement produces a
  `while (true)` nothing ever continues, "which looks like elimination and is
  not" (`tail_calls.rs`). So: a self-looping function gets an explicit
  `Loop`/`Continue` node in its body, a merged group becomes one function with a
  dispatch parameter, and every backend emits what it is given. The `Plan` type
  stays as the analysis; the emitters stop reading it.
- **`decision.rs`.** An arm chain tests arms in order, so reaching the last one
  costs O(arms) comparisons. A decision tree over the scrutinee's discriminants
  is O(1) for an enum match. It is the shape a `switch` wants in JavaScript and
  in LLVM, and the shape a tree of tests wants in an emitter with no instruction
  selection at all — one pass, three beneficiaries.
- **`closures.rs`.** `ExprKind::Lambda { captures }` already carries the capture
  list. Conversion turns a lambda into a top-level function taking an
  environment as its first parameter plus a construction of that environment. It
  is sound without analysis because SPEC 10.6 forbids capturing an
  effect-carrying value, so an environment is always plain immutable data.
- **`layout.rs`.** The value model of VALUE-MODEL.md, computed once per type and
  memoised. It sits in the middle end rather than in a backend because both
  native backends must agree byte for byte — an `[T]` whose element stride the
  debug and release backends disagree about is a miscompile that only shows up
  between profiles.

### 2.3 What the JavaScript backend loses and gains

Loses: the `Plan` consultation, `arm_chain`, and dead-code elimination by name.
Gains: decision trees, and closure conversion it will immediately undo.

Closure conversion is a pessimisation in JavaScript — an arrow function closing
over its scope is what the engine wants. So the JS backend gets the tree
**before** `closures` runs, and the native backends get it after:

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

Folding is not a stage of its own. It lives inside `inline`, interleaved with
it, because inlining a constructor into a projection is what makes most folding
possible and folding is what exposes the next round's call sites.

`derives`, `fuse` and `rc` run on the native branch only. `derives` generates a
`Show`, `Equal`, `Hash` and `ToJson`/`FromJson` per type where JavaScript walks a
descriptor at run time (VALUE-MODEL.md §9). `fuse` collapses a combinator chain
into one loop, deleting an intermediate list that costs `malloc` plus a copy
natively. `rc` inserts and elides reference-count operations, which a
garbage-collected target has no use for (MEMORY.md §5.2). `fuse` runs after
`derives`, so that a generated body's own chains fuse, and before `closures`,
because fusion composes the lambdas that `closures` is about to lift. Leaving
JavaScript unfused also leaves it as the reference the agreement tests compare
both native backends against.

`middle::run` is layer A, and `middle::native` is the branch after it. Both
mutate the program in place rather than returning a new one, because function
indices never move — `inline.rs`'s own invariant.

**The build system composes; a backend cannot ask.** `middle::native` needs the
program by `&mut` and `Backend::emit` gets it by `&`, which is the type saying
that a backend transforms nothing. The composition is one function,
`actions::prepare(program, target)`: layer A always, the native branch when the
target is not `Js`. Both callers that reach a backend go through it
(`actions::emit` for a text artifact, `actions::objects_of` for objects), so one
place decides which passes a target gets, and `backend::select` is the only
other thing the two paths share.

That seam costs one extra lowering on a native build: `middle::lower` runs once
in `objects_of`, whose `codegen` keys are hashes of the lowered IR, and once
inside `Backend::emit`. Lowering is a pure function of the program, so the two
agree by construction. The alternative — `emit_lowered` on the trait — buys one
lowering at the price of a second entry point the JavaScript backend cannot
implement and that every future caller could choose instead of the seam. At the
sizes the conformance corpus reaches, lowering is a small fraction of a native
build; measure before revisiting.

## 3. The `Backend` trait

```rust
/// One codegen unit's output. The backend computes `key`, not the build system:
/// only it knows which of its own inputs — target triple, LLVM version, pass
/// pipeline — the bytes depend on.
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

    /// Intrinsic keys this backend has no body for, so "missing intrinsic" is a
    /// question asked per backend.
    ///
    /// It takes `&Tables` because deciding whether a key has a body goes through
    /// the same code the emission does — `Gen::intrinsic`, which resolves
    /// `number.*` through the *type* of the function it is implementing — and that
    /// needs the type table. A version without it would be a second
    /// implementation of the question, and the two would drift.
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
  built inside it.
- **`Vec<Emitted>` even for JavaScript.** The JS backend returns exactly one
  element and its `Linker` is "take element zero". A special case would be a
  second code path through the build system.
- **`identity()` separate from `name()`.** `llvm-sys` links against whatever
  `llvm-config` found at build time, so two `buri` binaries with identical Rust
  source can have different LLVM underneath, and `Profile::Release` on LLVM 20
  must not share a cache entry with LLVM 21. The build system has no way to ask,
  so the backend answers.
- **`missing_intrinsics` takes the program**, not the `&[String]` an emitter
  accumulated as a side effect of emission. Asking up front means
  `buri build --output=linux/arm64` on a program using an unimplemented
  intrinsic reports it before spending a second in LLVM.
- **`emit_units` alongside `emit`, with a default.** Its absence was measurable:
  at 118k lines a one-line edit cost 2,622 ms, of which 64% was `emit`
  re-producing several hundred already-cached objects that were then thrown away
  (`design/PERFORMANCE.md` §6.5). The default forwards to `emit`, so a backend
  with one unit implements nothing.

`Profile` stays two-valued and lives in `backend/mod.rs`, because
`Profile::defensive_aborts` is a statement about programs and not about
JavaScript.

## 4. Backend selection

```
(Js | Web,       _)        -> js
(Linux | Macos,  Debug)    -> stencil
(Linux | Macos,  Release)  -> llvm
```

`Web` joins `Js` because the question this match asks is "which backend emits
this artifact", and a page is JavaScript.

There is no third native backend. CODEGEN-STENCIL.md §13 records the
retargetable one that held the debug seat until 2026-08-29, its removal, and its
costs; DECISIONS.md's three rows point at it.

The debug row is not every native triple. `stencil` carries one stencil library
per (instruction set, container) pair and refuses a target it has no library for,
so `select` returning it is necessary and not sufficient: `actions::native_ready`
still asks the backend. The libraries are `macos-arm64`, `linux-arm64` and
`linux-x86_64` (CODEGEN-STENCIL.md §3.2).

**macOS on x86-64 has no library, and it stays that way.** It is the one native
triple with no debug backend at all: `stencil::supported` refuses it by name —
*"the stencil backend has no stencil library for macos-x86_64"* — so
`native_ready` is false there, `driver::host_platform()` answers `Js`, and
`buri build` refuses with a sentence naming the target. A host with no `cc` gets
the same shape. CODEGEN-STENCIL.md §3.2 and §9 carry the argument.

The split was weighed against published measurements of a retargetable
generator's standing between LLVM `-O0` and a template JIT: Xu and Kjolstad's
*Copy-and-Patch Compilation* and Schwarz, Kamm and Engelke's *TPDE: A Fast
Adaptable Compiler Back-End Framework*, both linked from
[../../reference/README.md](../../reference/README.md). They are also why the
debug row could change hands on a measurement rather than on an opinion.

Two things sit on top of the table:

- **There is no `--backend` flag, and the agreement test does not need one.**
  Selection is `backend::select(target, profile)` and it takes no name, so what
  is being built chooses a backend rather than an argument a user passes. The
  cross-backend differential test is `cli/tests/native/agreement.rs`: it
  compiles one source twice from one analysis, through `actions::prepare` and
  `select` for each side, and compares stdout byte for byte. It is written
  against VALUE-MODEL.md §12's divergence table, one `#[test]` per row, so a
  failure names the row.
- A build of the toolchain without the `backend-llvm` feature refuses a native
  release build with a diagnostic naming the feature, rather than silently
  falling back to the debug backend. Falling back would mean `--release`
  produced different code depending on how the compiler was installed, which is
  the same class of bug as an unpinned toolchain. See BUILD-AND-WATCH.md §2.

`driver::host_platform()` is one line and a condition: `native_ready(host, Debug)`
— the host's own platform where this toolchain can produce something for it (a
backend compiled in, a runtime archive, a linker), and `Js` where it cannot.

**It does not decide the artifact**: `Output` does, and the build system reads
the *declared* outputs. `host_platform()` reaches the language server and the
documentation harness, where nothing varies on it. What *does* vary on the host
is `buri run`, which prefers a declared host-native output and executes the
artifact directly, and `buri test`, which runs a suite as a native binary
whether or not it named one.

**`buri test`'s default has flipped; `selected_outputs`' has not.** A suite that
names no platforms runs **natively**, in the dev profile. A binary that declares
no outputs still gets `JS`, because an artifact that silently changed platform
would change what `buri run` executes and what a release ships. That flip stays
what it is — one line, when the refusal goes quiet across the conformance corpus.

**A test suite is refused too, and that is the point.** The JavaScript fallback
is gone (buri-lang/buri#4). Rerouting is how a *named* gap becomes a wrong
answer: the suite passes, on a backend nobody chose, and what it proves is that
the other backend agrees with itself. So:

- A program the backend has no body for is `commands/test.rs`'s `native_gap`,
  asked of the monomorphized program before a second is spent on codegen, and
  reported as an error naming the intrinsic and the backend.
- A toolchain that cannot build for its own host — `--no-default-features`, a
  host outside macOS and Linux, no stencil library for the triple, no C
  compiler, or `--release` without `backend-llvm` — is `not_ready`, reported as
  `native-run-not-available` and naming the profile that was asked for.

The only two ways a suite reaches JavaScript are the two ways to say so out
loud: `--output=js` for an invocation, `test { platforms: [JS] }` for a suite.
The measured reason for spending the old fallback is `design/PERFORMANCE.md` §6:
the native dev loop is now the faster one on both halves of a 104k-line
edit-test cycle.

**A refusal names *which* of three things is missing.** `actions.rs`,
`commands/build.rs` and `commands/test.rs` are all gated on `native_ready`, and
one sentence covering all three causes was false for two of them: the *host*
refuses a `linux/x86_64` output on a mac that had just built `macos/arm64`, and
the *profile* refuses `--release` without `backend-llvm` on a toolchain whose
debug build of the same output works (buri-lang/buri#25, buri-lang/buri#26).
`build/actions.rs`'s **`native_gap`** asks the three questions in order and
answers which one failed, as an output, a reason and a fix; `native_ready` is
now "is there no gap"; and all three sites print it through one templated
diagnostic, `native-artifact-not-available`.
`repositories/cli/output_selection` pins the host half. The profile half cannot
be a golden, because what `--release` answers for the host's own target depends
on which leg of `cli/tests/README.md`'s bar the toolchain was built on.

**Those two cases now depend on one thing they did not:** the *host*. On a Linux
x86_64 machine `--output=linux/x86_64` is no longer refused — it builds — so both
fixtures pass on macOS and on a host whose platform they do not name, and fail on
the host they do. Fixing that needs the harness to know which platform is the
host and which is the cross one, since the refusal names a platform and the
golden is text. It is the one piece of golden work wave 3c did not do.

## 5. Codegen units

### 5.1 One unit per source module

A codegen unit is **the set of monomorphized functions whose declaration came
from one source module**. `Func::debug_name` is already `module:owner.name`
(`monomorphize.rs`), so the partition exists in the data; it becomes an explicit
`Func::unit: u32` assigned by `middle::lower`.

Three candidates were considered.

- **One unit per function.** Zig's self-hosted linker works this way and it
  gives the finest possible incrementality. Rejected on link cost: a
  conformance-sized program monomorphizes into thousands of functions, and a
  thousand-member archive is slower to link than the compile it saved. It also
  makes every intra-module call go through the linker.
- **A fixed count, merged.** What rustc does. Rejected because the merge is the
  part that hurts: two unrelated modules in one unit means an edit to either
  invalidates both, and the count is tuned for rustc's parallelism rather than
  for reuse.
- **One per source module.** Chosen. It is the unit an edit is scoped to, and a
  developer can predict the partition without reading the compiler.

The standard library is one unit per standard-library module on the same rule,
so a program that touches two functions of `core/list` pays for one `core/list`
object and reuses it across every build **of that target** — thirty modules that
essentially never change.

The reuse stops at the target rather than at the repository. Monomorphization
makes a unit's IR a function of the whole program it is in, so two binaries'
`core/list` objects are the same bytes only where neither instantiated anything
the other did not. Measured on a 118k-line repository with two native binaries
over one library: **2 of 369 codegen units** were shared across the pair, and
the cold `buri build //...` cell does not move when they are not (1.46 s against
1.49, one run each, inside the noise). That is why `unit_prefix` being a term of
the `codegen` key (§6.2) costs so little. A batched test binary spans packages
under one empty prefix and shares within itself, which is where the sharing that
matters happens.

A unit over a node budget (default 40 000 IR nodes) is split at function
boundaries into `foo.0`, `foo.1`, ..., deterministically by the existing function
order — which is source order, which is what `monomorphize.rs` already guarantees
for reproducibility.

### 5.2 The same partition in both profiles

Release does not merge units and does not use LTO. Cross-unit inlining is the
thing LTO exists to recover, and in this compiler inlining has already happened
— in the middle end, over an *exact* call graph, with no dynamic dispatch
anywhere in the language to blunt it. LTO would re-derive a worse version of a
decision already taken with better information. What LLVM contributes at release
is machine-level: instruction selection, scheduling, register allocation,
vectorization and the peepholes, all of which are function- or unit-scoped and
lose nothing to a unit boundary.

What is lost is inlining *into* the runtime's own functions, since `cli/runtime`
is a prebuilt archive. That is real, and it is bounded: the runtime's hot entries
(`incref`, `decref`, `alloc`) are not called, both backends open-code them
(MEMORY.md §5), so the archive contains only operations large enough that a call
is noise.

Release does two things Debug does not: every function not reachable from a root
gets internal linkage, so LLVM may specialize and delete it; and the middle end
runs with a larger inline budget (`inline::Options::rounds` goes from 3 to 6,
`SINGLE_USE` from 96 to 256).

If measurement later says ThinLTO is worth it, it is additive — a per-unit
bitcode emission and a second link step — and nothing here forecloses it.

## 6. The action graph

### 6.1 New actions

`cache::Action` gains one variant:

```rust
pub enum Action { Proto, Compile, Codegen, Link, Test }
```

`Codegen` is one action per codegen unit. `Compile` stays the front-end key that
`--explain` reports and nothing stores. `Link` stays the artifact key.

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

Keying a unit on the sources of the module it came from — the way
`actions::contribute` keys a target — is wrong in both directions. It is
*unsound*, because a monomorphized unit contains instantiations requested by
other modules: `core/list`'s object for a program depends on which types that
program maps over. And it is *imprecise*, because reformatting a comment in
`parse.buri` changes its bytes and not one instruction of its IR. Hashing the IR
fixes both — the IR is what codegen reads, and it is insensitive to everything
that is not semantics.

The IR is not *all* codegen reads, and the rest of `Options` is in the key for
the same reason: `profile`, `target`, and `unit_prefix`. The prefix is there
because §7 makes it reach the object, and because it already does on every ELF
target, where LLVM emits a unit's module name as a `.file` directive and
therefore as an `STT_FILE` symbol. It costs the cross-package reuse §5.1 counts
on: two targets whose closures share a unit compile it twice, because they are
two prefixes. A key that omits an input to codegen is a key that can serve bytes
codegen would not have produced, and that is the one thing this key exists to
rule out.

The cost is that computing the key requires running the front end and the whole
middle end, so `codegen` can never be skipped without doing the analysis. That is
nearly free here: `conformance build //...` measures 22 ms end to end, and the
expensive half of a native build is the half the key is protecting.

`Link`'s key is the ordered list of `codegen` keys plus the linker's identity:

```
link_key = H(Link, toolchain, mode, platform, arch,
             linker.name(), linker.version(),
             [codegen_key(u) for u in units],   // ordered
             runtime_archive_hash | "omitted")
```

Ordered, because link order determines symbol resolution order and therefore the
bytes. The last term is the archive's **decision** rather than its digest: the
link names `libburi_rt.a` only when the objects carry a `buri_rt_*` symbol
(BUILD-AND-WATCH.md §2.2), and a link that does not name it does not depend on
it. Two decisions are two command lines and therefore two keys; one term either
way, because an omitted archive has no digest to state. Both keys are built with
the existing `KeyBuilder`, which length-prefixes every field (`cache.rs`,
`Sha256::field`) so two different field decompositions cannot collide.

### 6.3 Where objects live

```
.buri/cache/<ab>/<rest>        the object bytes, content-addressed, as today
.buri/out/<platform>-<arch>/<pkg>/<artifact>          the executable
.buri/link/<link-key>/manifest                        unit name -> codegen key
.buri/link/<link-key>/<unit>.o                        hard link or copy from cache
```

The `.buri/link/<key>/` directory exists because a linker takes paths, not bytes,
and because the manifest is what makes "which objects changed" answerable without
re-running codegen. `Cache::get` returns bytes; the link step writes them into
the link directory under stable filenames, hard-linking where the filesystem
allows it.

The manifest is the input to CODEGEN-STENCIL.md §12.2's incremental relink, and
it is also what `--explain` reads to print one `codegen` line per unit with its
status. `buri clean` takes `.buri/link` with the rest.

`actions::artifact_path` already produces
`.buri/out/<output.dir()>/<pkg.path>/<name>` and `Output::dir()` already produces
`linux-x86_64`. The only change is that `Platform::Js => format!("{base}.mjs")`
gains no sibling: a native artifact's name is `base`, with no extension, which is
what `artifact_path`'s `_` arm already does.

## 7. `--check-reproducible` for a native artifact

`commands::build::check_reproducible` builds twice into two directories, from two
fresh sessions, with the cache off, and compares bytes. Three changes make it
work for an executable.

**It compares objects first, then the executable.** A byte offset into a four
megabyte executable names nothing a person can act on. Compared per unit, the
report is "`core/list.o` differs, first at byte 4192" — which names a module, and
a module names a pass. The executable is compared too, because a reproducible set
of objects and an irreproducible link is a real failure mode (link order, archive
member ordering, a temporary path in a debug section) and it is the one a
per-object comparison would hide.

**It runs codegen twice in one process rather than shelling out**, through
`actions::compile_artifact`. The native equivalent stops after `Backend::emit`
for the object comparison, then links both sets into the two round directories.

**Three sources of nondeterminism are closed at the source rather than compared
for.**

- **Mach-O `LC_UUID`.** Emitted with `-no_uuid` (`ld64.lld`) / `-Wl,-no_uuid`. A
  content-derived UUID would be reproducible; a random one is not.
- **Absolute paths in debug info.** `DW_AT_comp_dir` and, on Mach-O, the `N_OSO`
  stab entries name the object's path on disk. Both are set from
  `Options::unit_prefix`, which is repository-relative — the same rule
  `actions::action_key` already follows for input paths. This is precisely the
  failure the two-directory design exists to catch.
- **Timestamps in archive members and in the Mach-O/ELF headers.** Zeroed.
  `SOURCE_DATE_EPOCH=0` is already in the action environment (`build/spawn.rs`;
  `buri docs build/hermeticity`) and the object writer honours it directly rather
  than through the environment, since it is in-process.

The claim the flag earns is unchanged in wording and stronger in content: two
builds of the same commit produce identical bytes, and now the bytes are an
executable.

## 8. Implementation waves

The waves this section scheduled have all landed, and the schedule is not kept:
what it planned is now the module layout in §2, the trait in §3, and the action
graph in §6. What remains open is in the design notes, under "The native
backend".

## 9. What this does not do

- **No cross-*linking*.** Cross *codegen* works and is exercised. The debug
  backend bakes one stencil library per target and looks one up by triple rather
  than by the running CPU, cross-building both of its Linux libraries on a macOS
  host with no Linux sysroot (CODEGEN-STENCIL.md §3.2), and LLVM targets
  everything: the benchmark suite takes `aarch64-apple-darwin`,
  `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` as default rows on
  whichever machine it is run on (`design/PERFORMANCE.md` §3). A cross triple is
  in fact *more* reproducible than the host one, because the host ISA is inferred
  from the running CPU's features and a cross ISA is the baseline for its triple.

  What is still refused is producing a runnable artifact for another host:
  `buri build --output=linux/x86_64` on a macOS host is an error naming the host
  it can build for. The policy lives in one predicate, `link::can_link` — the
  target's platform and architecture must be the host's — with
  `actions::native_ready` in front of it. The reason is the runtime archive (§2,
  `cli/runtime`), which `cli/build.rs` builds for the host and for nothing else,
  and which a cross link would need alongside a cross libc and a sysroot. So the
  fix, when someone wants it, is "ship prebuilt runtime archives per triple": a
  packaging problem rather than a compiler one.
- **No dynamic linking, no shared libraries, no `dlopen`.** The language has no
  FFI to declare one with, so there is nothing to link against but the runtime.
  What "static" *means* differs on the two platforms:

  - **Linux: a static-PIE executable against a musl the toolchain carries.** Not
    the machine's libc, and not a distribution's musl either. `cli/build.rs`
    builds `libburi_rt.a` for `<arch>-unknown-linux-musl` and bakes eleven files
    out of that rustc's own `self-contained/` directory — `libc.a`,
    `libunwind.a`, and the crt objects — into the `buri` binary; `build/link.rs`
    writes them back out beside the objects and points the driver at them. So the
    libc an artifact carries is a property of the toolchain that built it and not
    of the machine that ran the build: a `buri build` on a 2024 distribution has
    to produce something that runs on a 2018 one. CODEGEN-STENCIL.md §12.3 has
    the command line and the three tiers.
  - **macOS: dynamically linked against libSystem, because there is no other
    option.** Apple ships no static libc and has not since 10.4, and a binary
    that bypassed `libSystem.dylib` to make raw syscalls would be one Apple has
    said it may break in any release. Every other dependency is still static.

  Three consequences follow from the Linux half:

  - **`dlopen` is not merely forbidden, it is absent.** That is also why a
    *statically linked glibc* was not the answer: glibc's `getaddrinfo` dlopens a
    matching `libnss_*.so` at run time, so a "statically linked" glibc artifact
    still needs a `libnss_files.so` of the right version present, and the thing
    the static link was for is exactly what it fails to deliver.
  - **Name resolution is musl's, which reads `/etc/resolv.conf` and `/etc/hosts`
    and nothing else.** There is no NSS, so mDNS, LDAP, NIS and
    `systemd-resolved`'s own plugin do not participate.
    `cli/runtime/http.rs`'s `resolve` calls `to_socket_addrs`, which is
    `getaddrinfo`, so a Buri program fetching a `.local` name — or a corporate
    name served only by an NSS module — will not find what a glibc program on the
    same machine finds. It is accepted because the alternative above does not
    work at all, and because the failure is a name that does not resolve rather
    than a binary that does not start.
  - **musl's `malloc` is slower than glibc's under multithreaded churn**, and the
    runtime allocates: `cli/runtime/lib.rs` §5 is `malloc`-backed, one block per
    allocation. What keeps that from being a per-value trip into a contended
    allocator is `cli/runtime/memory.rs`'s G2 per-thread block caches — a free
    returns the block to this thread's cache and the next allocation of that size
    takes it back without touching `malloc` at all. If it ever does bite, **the
    answer is to bundle an allocator into the runtime archive, not to go back to
    glibc**: a libc the artifact does not carry is the property this whole
    section is about.
- **No threads.** This is the language's decision rather than a native-backend
  one, and MEMORY.md §3 records what it buys: non-atomic reference counting.
