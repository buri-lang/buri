# Native backends: architecture

Two native backends, one middle end, and a link step that reuses object files.
LLVM through `inkwell` for `--release`; Cranelift through `cranelift-object` for
everything else. This document says where the code goes, what the interface
between the middle end and a backend is, how the build graph grows, and what
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
    closures.rs         lambda -> code pointer + environment record
    derives.rs          a generated Show/Eq/Hash/Json per type
    rc.rs               own/borrow inference, elision, reuse
    layout.rs           the value model, as a computed table
    ir.rs               block-argument SSA, native only
    lower.rs            tree -> ir
  backend/
    mod.rs              the `Backend` and `Linker` traits, and `Emitted`
    runtime_native.rs   the embedded `libburi_rt.a`, and its hash
    js/                 generate.rs  javascript.rs  intrinsics.rs  runtime.js
    cranelift/          mod.rs  emit.rs  helpers.rs  abi.rs  runtime.rs
    llvm/               mod.rs  emit.rs  repr.rs  attrs.rs  runtime.rs  target.rs
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
(`monomorphize.rs:8`), so every decision taken here is a fact rather than an
estimate — which is already the argument `optimize.rs:5-7` makes and is the
reason the pass sits where it does.

**Layer B — the CFG.** `middle::ir`, a per-function control-flow graph of basic
blocks with **block parameters**, produced by `middle::lower` from the layer-A
tree. Only the native backends consume it.

The JavaScript backend does not get a CFG, and that is the decision rather than
an omission. Going from a CFG back to structured JavaScript needs a relooper, and
a relooper would take a backend that today prints code a human can read
(`generate.rs:44-46` — debug builds stay readable, because the names are what
make a stack trace useful) and make it print a state machine. The gain would be
zero: everything JavaScript needs from the shared work is in layer A.

The counter-proposal — no layer B, and each native backend builds its own SSA —
is rejected on `design/LLVM-tips.md:2`. "Avoid mem2reg, generate optimized SSA form" is
an instruction the frontend can only honour by having SSA *before* it reaches
LLVM. Cranelift would build it for us (its `FunctionBuilder` runs the Braun
algorithm over `Variable` def/use), LLVM would not, and having one backend's SSA
be real and the other's be an artifact of `alloca` is exactly the divergence that
makes two backends disagree. Build it once, in the middle, in the block-argument
form — which is Cranelift's native shape and is a mechanical transliteration into
LLVM phis (see CODEGEN-LLVM.md §2).

### 2.2 What each new layer-A pass is for

- **`dce.rs`.** `design/LLVM-tips.md:1`: dead code elimination before it reaches LLVM
  IR. Monomorphization already gives reachability-based DCE for free
  (`monomorphize.rs:12-14`), but inlining creates *new* dead functions — a body
  inlined at its single call site leaves the original unreachable, which
  `optimize.rs:11-12` says out loud and then leaves for `javascript::eliminate_dead`
  to drop by name. Dropping by name is a JavaScript minifier's job. A native
  backend needs it dropped by index, before layout and codegen spend time on it.
  So DCE moves into the middle end and the minifier stops being where it happens.
- **`tail_calls.rs` rewrites.** Today it produces a `Plan` the emitter consults
  (`tail_calls.rs:53-77`), and the emitter has to agree with it about what tail
  position is — `tail_calls.rs:22-27` records that a disagreement produces a
  `while (true)` nothing ever continues, "which looks like elimination and is
  not". Two implementations of one rule is one too many at one backend and three
  too many at three. It becomes a rewrite: a self-looping function gets an
  explicit `Loop`/`Continue` node in its body, a merged group becomes one
  function with a dispatch parameter, and every backend emits what it is given.
  The `Plan` type stays as the analysis; the emitters stop reading it.
- **`decision.rs`.** `arm_chain` (`generate.rs:1429`) tests arms in order, which
  is O(arms) comparisons to reach the last one — `generate.rs:1453-1458` says so
  and offers a release-mode shortcut that only helps the final arm. A decision
  tree over the scrutinee's discriminants is O(1) for an enum match and is the
  shape a `switch` wants in JavaScript, a `br_table` wants in Cranelift, and a
  `switch` wants in LLVM. One pass, three beneficiaries.
- **`closures.rs`.** `ExprKind::Lambda { captures }` already carries the capture
  list. Conversion turns a lambda into a top-level function taking an environment
  as its first parameter plus a construction of that environment. It is sound
  without analysis because SPEC 10.6 forbids capturing an effect-carrying value,
  so an environment is always plain immutable data.
- **`layout.rs`.** The value model of VALUE-MODEL.md, computed once per type and
  memoised. It is in the middle end rather than in a backend because both native
  backends must agree byte for byte — an `[T]` whose element stride Cranelift and
  LLVM disagree about is a miscompile that only shows up between profiles.

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
               js            derives -> closures -> rc -> layout -> lower -> ir
                                                                              |
                                                            +-----------------+-------------+
                                                            |                               |
                                                       cranelift                           llvm
```

Folding is not a stage of its own: it is inside `inline`, interleaved with it,
because inlining a constructor into a projection is what makes most folding
possible and folding is what exposes the next round's call sites. That is why
`optimize.rs` becomes `inline.rs` rather than `inline.rs` plus `fold.rs`.

`derives` and `rc` are on the native branch only. `derives` generates a `Show`,
`Eq`, `Hash` and `ToJson`/`FromJson` per type where JavaScript walks a descriptor
at run time (VALUE-MODEL.md §9); `rc` inserts and elides reference-count
operations, which a garbage-collected target has no use for (MEMORY.md §5.2).

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

pub trait Backend {
    /// `js`, `cranelift`, `llvm`. Enters every cache key this backend produces.
    fn name(&self) -> &'static str;

    /// The identity of everything outside the program that the bytes depend on:
    /// the LLVM version, the Cranelift version, the runtime's own hash. Enters
    /// every cache key. A backend that returns a constant here is claiming its
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

Four things about this signature are decisions:

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
  effect (`generate.rs:2669-2677`, fed from `generate.rs:279`), so a program can
  only be told what it is missing *after* a failed emission. Asking the backend
  up front means `buri build --output=linux/arm64` on a program using an
  unimplemented intrinsic reports it before spending a second in LLVM.

`Profile` stays two-valued and stays where it is (`generate.rs:36`). It moves to
`backend/mod.rs`, because `Profile::defensive_aborts` (`generate.rs:54`) is a
statement about programs and not about JavaScript, and it grows nothing.

## 4. Backend selection

```
(Js,             _)        -> js
(Linux | Macos,  Debug)    -> cranelift
(Linux | Macos,  Release)  -> llvm
```

The published measurements this split was weighed against — Cranelift's standing
between LLVM `-O0` and a template JIT — are vendored at
[../../reference/xu-kjolstad-copy-and-patch.pdf](../../reference/xu-kjolstad-copy-and-patch.pdf)
and [../../reference/tpde-fast-compiler-backend.pdf](../../reference/tpde-fast-compiler-backend.pdf).

Two knobs on top:

- `--backend=<name>` forces one, and is how `cranelift_and_llvm_agree` is
  written — the native analogue of the existing `release_and_debug_agree`
  (`generate.rs:52-56`). Two backends that are never run over the same program at
  the same profile will disagree, and the disagreement will be found by a user.
- A build of the toolchain without the `backend-llvm` feature refuses a native
  release build with a diagnostic naming the feature, rather than silently
  falling back to Cranelift. Falling back would mean `--release` produced
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
runs a suite named for a native platform as a native binary.

**The default output is still `JS`, and that is a decision rather than the
remaining half of this one.** A binary that declares no outputs gets `JS`; a
suite that names no platforms runs on JavaScript. The reason is the runtime
surface rather than the backend: `Backend::missing_intrinsics` refuses a program
reaching `core/fs`, `core/env`, `json.*` or any `list.*` entry taking a closure,
which is the right refusal and the wrong default — a `buri run` that fails on a
program `buri run` used to run is not an improvement. The condition for flipping
it is that refusal going quiet across the conformance corpus, and when it does,
the change is `selected_outputs`' fallback and `run_suite`'s.

`actions.rs` and `commands/build.rs` and `commands/test.rs` each refused a non-JS
platform with "the backend is not implemented". All three are now gated on
`native_ready` instead, in the wave that owned each (2c, 3a, 3c) — the wording is
unchanged, because a toolchain that cannot produce a native artifact must say the
same thing it always said, and that is what `repositories/cli/output_selection`
and `repositories/testing/suite_platforms` pin.

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
(`monomorphize.rs:316`), so the partition exists in the data; it becomes an
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
object and reuses it across every build in the repository. That is a large,
free win: the standard library is thirty modules that essentially never change.

A unit over a node budget (default 40 000 IR nodes) is split at function
boundaries into `foo.0`, `foo.1`, ..., deterministically by the existing function
order — which is source order, which is what `monomorphize.rs:247-248` already
guarantees for reproducibility.

### 5.2 The same partition in both profiles

Release does not merge units and does not use LTO. The reasoning:

Cross-unit inlining is the thing LTO exists to recover, and in this compiler
inlining has already happened — in the middle end, over an *exact* call graph,
with no dynamic dispatch anywhere in the language to blunt it
(`monomorphize.rs:8-10`). LTO would be re-deriving a worse version of a decision
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
end runs with a larger inline budget (`optimize::Options::rounds` goes from 3 to
6, `SINGLE_USE` from 96 to 256).

If measurement later says ThinLTO is worth it, it is additive — a per-unit bitcode
emission and a second link step — and nothing here forecloses it.

## 6. The action graph

### 6.1 New actions

`Action` (`cache.rs:189-199`) gains one variant:

```rust
pub enum Action { Proto, Compile, Codegen, Link, Test }
```

`Codegen` is one action per codegen unit. `Compile` stays what it is — the
front-end key that `--explain` reports and nothing stores (`actions.rs:226-229`).
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
                      H(the unit's lowered IR),
                      H(the layout of every type the unit names))
```

This is the decision the whole incremental story rests on, so it is worth being
explicit about why it is not the obvious thing. The obvious thing is to key a
unit on the sources of the module it came from, the way `contribute`
(`actions.rs:184-221`) keys a target. That is wrong here in both directions.
It is *unsound*, because a monomorphized unit contains instantiations requested
by other modules — `core/list`'s object for a program depends on which types that
program maps over — and it is *imprecise*, because reformatting a comment in
`parse.buri` changes its bytes and not one instruction of its IR.

Hashing the IR fixes both. The IR is what codegen reads, so hashing it is hashing
the input; and it is insensitive to everything that is not semantics, so a
whitespace edit produces an identical key and the object is reused.

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
             runtime_archive_hash)
```

Ordered, because link order determines symbol resolution order and therefore
determines the bytes. Both keys are built with the existing `KeyBuilder`, which
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

The manifest is the input to CODEGEN-CRANELIFT.md §6's incremental relink, and it
is also what `--explain` reads to print one `codegen` line per unit with its
status. `buri clean` takes `.buri/link` with the rest.

`artifact_path` (`actions.rs:375-391`) already produces
`.buri/out/<output.dir()>/<pkg.path>/<name>` and `Output::dir()`
(`buildfile.rs:188-199`) already produces `linux-x86_64`. The only change is that
`Platform::Js => format!("{base}.mjs")` gains no sibling: a native artifact's
name is `base`, with no extension, which is what `actions.rs:386-389` already
does for the `_` arm.

## 7. `--check-reproducible` for a native artifact

`check_reproducible` (`commands/build.rs:147-262`) builds twice into two
directories, from two fresh sessions, with the cache off, and compares bytes
(`build.rs:135-146` states why each of those three is load-bearing). It refuses
non-JS platforms today at `build.rs:169-173`.

Three changes.

**It compares objects first, then the executable.** `first_difference`
(`actions.rs:152-158`) reports a byte offset, and a byte offset into a four
megabyte executable names nothing a person can act on. Compared per unit, the
report is "`core/list.o` differs, first at byte 4192" — which names a module, and
a module names a pass. The executable is compared too, because a reproducible set
of objects and an irreproducible link is a real failure mode (link order, archive
member ordering, a temporary path in a debug section) and it is the failure mode
a per-object comparison would hide.

**It runs codegen twice in one process rather than shelling out.** `compile_artifact`
(`actions.rs:109-143`) is already split out for exactly this. The native
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
  `action_key` already follows for input paths (`actions.rs:160-161`: "paths are
  repository-relative, so two checkouts in different directories produce
  identical keys"). This is precisely the failure the two-directory design exists
  to catch (`build.rs:144-146`), so it must be closed rather than tolerated.
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

- **No cross-compilation**, and the refusal is explicit:
  `buri build --output=linux/x86_64` on a macOS host is an error naming
  the host it can build for. The reason is the runtime archive (§2, `cli/runtime`),
  which is built for the host by `cli/build.rs` and for nothing else. Cranelift
  can target any triple it was built with and LLVM can target all of them, so the
  refusal is about the runtime and not about the backends — which means the fix,
  when someone wants it, is "ship prebuilt runtime archives per triple", and it
  is a packaging problem rather than a compiler one. Saying that is the point of
  refusing out loud.
- **No dynamic linking, no shared libraries, no `dlopen`.** Every artifact is a
  static executable against the platform libc. The language has no FFI to declare
  one with, so there is nothing to link against but the runtime.
- **No threads.** This is not a native-backend decision, it is the language's,
  and MEMORY.md §3 records what it buys: non-atomic reference counting.
