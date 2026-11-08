# Compiler pipeline

**What `buri build` turns a program into, step by step, what each step costs,
and which steps could go.** Written for somebody about to make the compiler
faster.

Measured 2026-11-07 at `b28b68747` on an M3 Pro. The input is the `full` goals
workspace (`cli/benches/workspace.rs`, `design/PERFORMANCE.md` §6.82): 48
libraries, 12 native programs and 4 pages, 262,368 lines of code once the stdlib
it reaches is counted. Every build is cold, with `BURI_THREADS=1` and the
`bench`-profile `buri` built with `--features backend-llvm`.

```text
source ──lex──▶ tokens ──parse──▶ syntax tree ──check──▶ Checked (tables + typed tree)
                                                            │   one per build, shared by every program
                                     ┌──────────────────────┘
                                     ▼   once per program
                               monomorphize ──▶ Program (typed trees, concrete)
                                     │
        inline · dce · tail_calls · decision · chunks           every target, in place
        derives · fuse · closures · forward · name_discards     native only, in place
        rc::analyze ──▶ Plan                                    analysis, side table
                                     │
               ┌─────────────────────┴───────────────┐
               ▼ pages                               ▼ native
        JS AST ─▶ minify ─▶ text            lower ──▶ ir::Program (SSA CFG)
                                                     │
                                        render + hash every unit (cache keys)
                                                     │
                             ┌───────────────────────┴──────────────────────┐
                             ▼ dev                                          ▼ release
                 stencil regions ─▶ object model ─▶ bytes      LLVM module ─▶ O2 ─▶ ISel ─▶ bytes
                             └──────────────────────┬──────────────────────┘
                                                    ▼
                                         cache file per unit ─▶ lld ─▶ executable
```

Dev and release share everything above the last split. Release spends 98% of
its CPU below it, inside LLVM.

## Representations

Sizes per line are peak-RSS deltas from `cargo bench -- --rss` on
`pinned:mixed-100k`, so each includes whatever its phase allocates alongside it.

- **Source text.** `buri_diagnostics::SourceMap`, one `Arc<str>` per file. Kept
  for the whole process.
- **Tokens.** `buri_syntax::parsing::lexer::Tokens`, a struct of arrays: a
  12-byte `Record` per token (kind, span, payload) plus side tables for wide
  literals and cooked strings. About 7 tokens and 121 B a line. Dropped once
  the file parses. Lint lexes the workspace's own files again.
- **Syntax tree.** `tree::Module` holds the declarations; everything below them
  lives in `flat::Tree`, `u32`-addressed arenas in post-order with 24-byte
  nodes. A name is its span. About 85 B a line. Kept for the whole process,
  one `Arc` per file in `parser::Cache`.
- **Checked.** `buri_semantics::resolve::Checked`: dense id tables
  (`types::Tables`, partly layered over the stdlib snapshot), scopes, and
  `bodies: IdMap<FnId, Arc<typed::Body>>`. A typed body is a boxed tree:
  `typed::Expr` is 104 B with `Box<Expr>` and `Vec<Expr>` children. `Ty` is
  interned. About 650 B a line. One per build, kept until the last program is
  built. There is no separate resolved tree; resolution and inference fill the
  tables and build the typed tree in one traversal.
- **Monomorphized program.** `buri_middle::monomorphize::Program`: a
  `Dense<FuncIdx, Func>`, each `Func` owning a concrete copy of a typed tree.
  One per program, 22,000 to 37,000 functions each, 335,011 across the build.
  Ten passes rewrite it in place. Dropped after emission.
- **Ownership plan.** `rc::Plan`, a `Dense<FuncIdx, FuncPlan>` of increment and
  release sites keyed by preorder `NodeId`. A side table, not a program.
  Consumed by `lower`.
- **IR.** `buri_middle::ir::Program`: per function a block-parameter SSA CFG
  (`Vec<Block>` of 40-byte `Inst`s), one interned `TypeList`, and a unit
  partition. Built per function in parallel, then the type lists merge. Kept
  until the program's objects exist. The monomorphized program, the plan and
  the IR together peak at about 1.5 KB a line.
- **Unit text.** `actions::unit_hashes` renders every function of a unit as
  text, plus the `Debug` text of every layout it reaches, only to hash it for
  the cache key. Dropped at once.
- **Dev objects.** `stencil::region::Emitted` per 512-function part (code, pool
  and relocations), then `object::Section`, `Symbol` and `Reloc` per unit, then
  Mach-O or ELF bytes.
- **Release objects.** One inkwell `Module` per unit, which LLVM optimizes in
  place and lowers to machine functions and object bytes.
- **Page output.** `buri_js::javascript::Stmt`, a boxed JS tree, minified into a
  new tree and printed.

## Passes

CPU shares are of the cold dev build's 7.39 CPU-seconds, child processes
included, from `samply` weighted by thread CPU time. "Whole" means every
function reached from the program's roots.

- **Lex.** Text to tokens. One linear scan of the whole file before parsing
  starts. Parse and lex together: 0.8%.
- **Parse.** Tokens to syntax tree. Recursive descent, linear, no feedback from
  resolution. Every file the build reaches, once.
- **Check.** Syntax tree to `Checked`. Resolution in phases (declarations,
  imports, signatures, impls), then local bidirectional inference per body,
  with no fixpoint. Linear in practice. Runs once per build over every module
  any target reaches (§6.97). Style extraction then rewrites the bodies in
  place, and again per program in `Checked::restyle`. In a test build that
  takes what the first run found wherever it answers the same. 3.0%, of which
  styles are 1.1%.
- **Monomorphize.** `Checked` to `Program`. A worklist from the roots, so only
  what's reached. Each instance clones its generic body
  (`typed::Body::clone`, 24% of the pass) and then rewrites the copy in place
  (60%). That's two walks per instance. Once per program, so a library is
  monomorphized again for each program that reaches it. 7.3%.
- **Inline.** In place. Call-graph SCCs (Tarjan), bottom-up, folding while it
  pastes; `Own::of` walks every pasted body. Whole program. 5.9%.
- **DCE.** In place. Reachability from the roots over the call graph. 1.4%.
- **Tail calls.** In place. SCCs of the tail-call graph; self-loops become
  loops, groups merge into one dispatcher. 0.1%.
- **Decision.** In place. Each `match` becomes a decision tree (Maranget). 0.8%.
- **Chunks.** In place. Reachability from `core/lazy` loads. Under 0.1%.
- **Derives.** Native only, in place. Generates one function per type per
  derived operation. 1.7%.
- **Fuse.** Native only, in place. Composes `core/list` combinator chains. 0.5%.
- **Closures.** Native only, in place. Lifts lambdas into functions with an
  environment. 0.6%.
- **Forward.** Native only, in place. Replaces field-path `let`s with the path.
  0.4%.
- **`rc::name_discards` and `rc::analyze`.** Native only. Names discarded
  values (in place), then builds the plan: a whole-program fixpoint over
  ownership and purity, plus about six walks of every body (`BodyIndex`,
  `effects_once`, `ownership_dependencies`, `collect_consuming`,
  `consuming_uses`, `scan_func`). 7.7%.
- **Lower.** `Program` plus `Plan` to IR, each function in parallel. It
  rebuilds the plan's preorder numbering (`Sites::of`) and builds its own
  `rc::Syntactic` for list loops. 6.4%.
- **Unit keys.** IR to text to SHA-256, per unit, plus each unit's layout
  closure rendered with `Debug`. Text formatting is most of it; the hash is
  not visible. 4.4%.
- **Stencil emission.** IR to bytes. Linear stencil selection per instruction
  (`Jit::function`), parts in parallel, then `assemble_unit` concatenates parts
  and writes the object. 18.3%, of which assembly is 2.5%.
- **LLVM emission.** IR to LLVM modules, `default<O2>`, instruction selection.
  Each unit's module is built once and reports the glue it asks for; the glue
  unit is built last, from those asks (§6.88). A unit the cache already holds
  is still built, without optimizing, whenever the glue unit isn't cached.
  Release only.
- **JS emission.** For the 4 pages: generate a JS tree, then `minify` folds,
  cleans, merges, drops dead code and mangles, each a full rewrite, then
  prints. The fold and dead-code passes repeat work `inline` and `dce` did on
  the typed tree. 8.8%, of which `minify` is 4.7%.
- **Cache writes.** One file per unit: open, write, `setxattr`, rename. 7.3%.
- **Link.** `lld` as a child process. 16.4%.
- **The rest.** Lint 2.0%, dropping typed trees 1.4%, the loader 0.9%, and
  3.9% spread too thin to name.

Every middle-end pass walks every function body again. None of them walks only
what the previous one changed.

## How many transformations

A native build rebuilds the program into a new representation nine times in
either profile, and updates one in place eleven times.

- **Rebuilds, both profiles.** Lex, parse, check, monomorphize, lower, unit-key
  rendering, link.
- **Rebuilds, dev.** Stencil regions, then object assembly.
- **Rebuilds, release.** The LLVM module, then instruction selection.
- **In place.** Style extraction, inline, dce, tail_calls, decision, chunks,
  derives, fuse, closures, forward, name_discards. Release adds LLVM's O2.
- **Whole-program analyses that build no program.** `rc::Syntactic` (built
  twice in dev, three times in release), `rc::analyze`, `Sites::of`, layouts,
  stencil frame signatures, LLVM's `observe`.

## Measurement

```text
cargo bench -p buri --bench compiler --features backend-llvm -- --goals=time
BURI_PROFILE=1 buri build //...                     # in target/goals/full, BURI_THREADS=1
BURI_PROFILE=1 buri build --release //...
samply record --save-only --unstable-presymbolicate -- buri build //...
```

Cold dev build, `BURI_PROFILE`, 6.0 s in-process plus 1.3 s of `lld`, with
`lower` billed to `middle` and unit keys to `emit`:

| Phase | CPU | Share |
|---|---:|---:|
| lex+parse | 0.048 s | 0.7% |
| check | 0.168 s | 2.3% |
| monomorphize | 0.531 s | 7.2% |
| middle | 2.060 s | 28.0% |
| emit | 2.253 s | 30.6% |
| link, with `lld` | 1.392 s | 18.9% |
| other | 0.910 s | 12.4% |

Cold release build: 294.3 s in-process plus 1.1 s of `lld`. `emit` is 290.3 s,
98.6%. Inside it `default<O2>` takes 50.5%, instruction selection to bytes
46.0%, and building the modules 1.7%. Everything above `emit` costs the same
as in dev.

A glue pre-pass used to build every module a second time, 1.6% (4.75 s).
Building each module once took a cold release build from 3,538 G
instructions to 3,458 G (−2.3%) and from 342.5 to 326.5 CPU-seconds, on a
loaded machine. The 24 outputs are byte-identical.
`profile::a_release_build_builds_each_llvm_module_once` holds `llvm modules
built` to `objects compiled`.

`other` is cache writes (0.54 s), lint, and the loader. It used to hold 0.83 s
of codegen too, because `lower::run_with` and `unit_hashes` ran outside any
phase. `profile::lowering_and_unit_keys_are_billed_to_codegen_not_other`
keeps them out.

Throughput against the goals, `--goals=time`, fastest of 3:

| Phase | Lines/CPU s | Goal | Of goal |
|---|---:|---:|---:|
| parse | 5.58 M | 10 M | 56% |
| check | 1.55 M | 1 M | 155% |
| dev codegen | 42.2 k | 100 k | 42% |

Dev codegen read 49.1 k before `lower` and the keys were billed to it.

On Linux the link was a third of dev codegen. One mold link of a `monorepo`
program retired 275 M instructions, and three things in it did no useful work:

- `-L` directories. nix's wrappers name each one three times or more, and
  the linker searches every copy for every library. `Replay` now keeps the
  first `-L<dir>` of each: 13 M per mold link, 59 M of 784 M per `ld64.lld`
  link.
- mold's fork. It links in a child and hands the output over while the child
  frees its memory. `-Wl,--no-fork` rides with `-fuse-ld=mold`: 49 M.
- DWARF. An ELF link copies the runtime's, std's and musl's debug sections
  into every artifact, mostly for code `--gc-sections` dropped. `ld64` never
  copies them. `-Wl,--strip-debug` saves 47 M and 3.3 MB of a 5.7 MB
  artifact. `build_driver::a_linux_artifact_carries_no_debug_information`
  holds it for each linker.

Unit keys memoize each type's layout line per worker, rather than
re-rendering the closure of every unit's types. Together the four cut dev
codegen instructions per line by 17% on `ci`, 18% on `monorepo-ci` and 14% on
its test build (`--goals=count`, aarch64 Linux).

`full` has no generators. A build's only child processes besides `lld` are
generator runs, one per generator rule, with a workspace tool under bun. The
`monorepo` goals workspace has 31 of them, and checks, compiles and runs
generators the way an application monorepo does (`design/PERFORMANCE.md`
§6.82).

Check passed its goal when §6.97 stopped checking each library once per
program. Dev codegen is short because it runs once per program. Each program
reaches 104k to 166k lines, so the back half compiles about 1.65 M lines for
262k unique ones. Measured per program reached, `cmd/p11` monomorphizes,
runs the middle end and emits at 470k lines a CPU-second.

A test build checks the same way: every suite the cache left to build, and
the lint's targets, are one compilation, and each suite restyles over its own
modules (`design/PERFORMANCE.md` §6.104). A restyle takes what the shared
compilation's own extraction found for each declaration wherever the
questions it asked of the program answer the same in that suite, and walks
only the rest (§6.106). On the `monorepo` workspace that took test-build
check from 339 k to 707 k lines a CPU second.

**Most per-program work is repeated.** The 12 linked executables define
203,858 functions between them, and only 63,313 distinct symbols (`nm -U`).
5,566 symbols appear in all 12. Not one of the 3,231 codegen-unit keys
repeated across programs until units got the same key in every program; now
2,481 come back from the cache (see
[Sharing codegen across programs](#sharing-codegen-across-programs)).

## Proposals

Ranked by CPU saved per unit of complexity. Savings are of the cold builds
above.

1. **Make codegen units the same across programs.** Done: 15% of dev CPU and
   76% of release; see
   [Sharing codegen across programs](#sharing-codegen-across-programs).
2. **Hash unit keys from the IR, not from its text.** About 4% of dev, 0.3 s
   in release. Evidence: `unit_hashes` is 0.32 s, and formatting
   (`render_func_into`, `fmt::write`, `Layout`'s `Debug`) is most of it.
   Feed a streaming hasher from each function as `lower_one` builds it, and
   hash layouts by field rather than by `Debug` string. Risk: low; the hash
   must spell types by module path, not `TyConId`, as §6.97 requires. It takes
   the text renderer out of the build; tests keep using it.
3. **Write a program's objects as one cache entry.** Up to 5% of dev. Evidence:
   0.54 s goes to one open, write, `setxattr` and rename per unit, 3,231 times.
   Write one pack per program with an index of unit keys, and keep reading per
   unit. Risk: medium; the cache layout, `buri clean` and the cache's own
   tests change.
4. **Instantiate while copying.** About 1.5% of dev. Evidence: `monomorphize`
   clones each body whole (24% of the pass) and then walks the copy to
   substitute (60%). One walk that builds the substituted tree from the
   borrowed generic one does both. Risk: medium; `rewrite` is large, and
   instance symbols must not change.

Smaller ones, each under 1.5%:

- Assemble stencil parts straight into the object's section buffers rather
  than concatenating and copying again: part of `assemble_unit`'s 2.5%.
- Hand `middle::native`'s `rc::Syntactic` to `lower` and to LLVM instead of
  rebuilding it; §6.42 already names this. A field on `rc::Plan`.
- Key the plan by the node itself rather than a preorder `NodeId`, so `lower`
  stops renumbering every body (`Sites::of`, about 0.2%).
- Check how much `minify`'s fold and dead-code passes still find after
  `inline` and `dce`; if little, drop them for pages (up to 4.7% here, only
  with pages in the build).
- Keep each workspace file's tokens for lint instead of lexing it again
  (0.1%).

Not worth proposing:

- **Lowering during checking.** Checking runs once per build; monomorphizing
  and lowering need one program's roots and run per program. Fusing them would
  undo §6.97's sharing.
- **Fusing the middle end's tree walks.** §6.42 tried it in `rc`: it saved a
  walk and no instructions, because the per-node match costs, not the
  traversal. The six small passes together are 2.4%.
- **An arena for the typed tree.** §6.52 measured it at 3–4% after the
  allocator change, under its 5% bar. Clones and drops of typed trees are still
  about 5% of dev here, so it may be worth measuring again once proposal 4
  removes the biggest clone.
- **A lower `-O` for release.** The release backend stays at `default<O2>`.
- **A flat libSystem stub for macOS links.** `ld64.lld` spends much of a link
  parsing the SDK's `.tbd` files, but SDK 27's tbd lists arm64e only, so a
  stub we write would have to track each SDK.

## Sharing codegen across programs

**Most per-program codegen is identical, and two small changes let the unit
cache we already have reuse it.** Both have landed; [Landed](#landed) has the
numbers. Measured 2026-11-07 on the `full` workspace
above, cold, `BURI_THREADS=1`, with a `validate`-profile `buri` and a
throwaway patch. The patch dumped each function's rendered IR hash, its stencil
bytes and relocations, and each unit's LLVM IR before and after `default<O2>`.

### What's identical

The share is of the copies a build compiles beyond the first, so it's what
compiling each function once would remove. Time is the per-function stencil
time in dev, and each unit's `default<O2>` plus instruction selection, split
across its functions by instruction count, in release.

| | Dev, functions | Dev, time | Release, functions | Release, time |
|---|---:|---:|---:|---:|
| Copies of another program's symbol | 74.7% | 82.0% | 73.9% | 83.9% |
| Identical copies, today | 67.9% | 54.1% | 48.4% | 60.5% |
| Identical copies, closures numbered per parent | 84.1% | 83.7% | 84.7% | 83.9% |
| Whole units identical, today | 58 of 3,231 | 0.1% | 69 of 3,231 | 0.1% |
| Whole units identical, closures numbered per parent | 2,521 | 76.2% | 2,544 | 79.9% |

Identical IR gives identical code. In dev, all but 190 functions match once
relocated fields are masked; those few most likely read a callee's frame
layout. Raw bytes differ for about 3,000 more, because constant-pool offsets
and part-local helper names (`buri$stencil$p{part}h{i}`) depend on the 512-function
part a function lands in. In release, identical IR gives identical IR before
`default<O2>` for every function except the per-unit `buri.thunk.N` helpers,
and 96% of those functions are still identical after it.

### Why the rest differ

- **Closure symbols.** `closures::run` names a lifted lambda
  `{parent}$fn{n}`, with `n` counted from the program's function count, so
  `heft25$1n9hh3$fn2204` in one program is `$fn2300` in another. Of the 3,646
  shared symbols whose IR differs, 3,569 differ only by that. Every caller of a
  closure differs with it, and so does the unit around it. It also renames every
  closure in a program when one function is added anywhere, so an edit misses
  every unit that holds a closure.
- **Single-use inlining.** `inline` pastes a callee with one call site by the
  program's own counts: 59 symbols.
- **Dead bodies.** 18 functions are `abort "this function was never built"` in
  programs that don't reach them.
- **Which functions a program reaches.** A unit holds every function the
  program reaches in one module. The workspace's library units match because
  every program reaches the same parts of each library. The stdlib's don't:
  `core_list` has 12 versions for 12 programs, and 1.1% of stdlib unit time
  repeats, against 84.7% function by function. The stdlib is only 2% of this
  workspace's emission, so a stdlib-heavy repository looks more like that row.

Monomorphization context, layouts and ordering caused no differences. Instance
symbols already carry their type arguments' hash.

### What blocked equal keys

Each of these is fixed except the partition.

- **`codegen_key` takes the package path** (`unit_prefix`), so two programs in
  two packages never share a key. Only the LLVM backend reads it, as the module
  name, which ELF writes as an `STT_FILE` symbol. The stencil backend ignores it.
- **Closure numbering** changes the unit's IR text, as above.
- **The partition is per program and module**, so a unit's key covers every
  function the program reaches in that module.

The rendered text in the key costs time (proposal 2), but it doesn't stop
matches: it's built from content, not from ids.

### The real saving

Measured, not simulated: the same build with both changes against without,
cold, CPU-seconds including `lld`.

| | Today | Both changes | Saved |
|---|---:|---:|---:|
| Dev | 7.85 s | 6.42 s | 18% |
| Release | 308.8 s | 86.8 s | 72% |

2,481 of 3,231 units come back from the cache. Dev saves less than the 39%
upper bound because monomorphize, the middle end, `lower` and unit keys still
run once per program. With lint and the loader they're 3.8 s of the 6.4 s
left. Release saves more than the 68%
the symbol count suggested, because a matched unit skips all of its LLVM work.

`buri test //...` on this workspace spends 0.02 s in emit, so there's nothing
there to share. Test binaries already have an empty prefix and share within a
batch.

### Release and inlining

**Release has no link-time optimization.** Each unit is its own module,
optimized by `default<O2>` alone. Other units' functions are declarations,
except small leaf glue kept `available_externally`. Whole-program inlining
happens earlier, in the middle end's `inline`, so the IR already carries it and
an identical IR is an identical final object. Release shares objects directly;
there's no bitcode stage to share.

Sharing whole units loses nothing. A finer split would cut `default<O2>`'s
inlining wherever a program-specific function calls a shared one in the same
module. Today that's 56% of its same-unit inlines (65,040 of 115,928). With
closures numbered per parent it's 0.6% (711), because the shared functions
then call almost nothing that isn't shared.

### Options

- **Make today's unit keys match.** Number lifted closures within their parent,
  name LLVM modules by unit alone, and drop `unit_prefix` from `codegen_key`.
  Saves 18% of dev and 72% of release here. Three small changes; the prefix
  test `the_codegen_key_carries_the_unit_prefix` goes. The first program writes
  each unit and the rest restore it, so it's done once per build through the
  cache we already have, and `Flights` covers programs built side by side. The
  key stays exact: it already holds the IR, the layouts, the backend, the mode,
  `named elsewhere`, `shares-counts` and the test entry, and nothing in an
  object reads the prefix once the module name doesn't. If debug info ever
  writes `DW_AT_comp_dir`, the prefix goes back into the keys of the units that
  carry it.
- **Emit each function once per build, in dev.** Memoize stencil emission by
  function: the IR hash, the layouts it names, its callees' frame layouts,
  `shares-counts` and the backend. Assemble each unit's object from the
  memoized parts, so objects stay per program and module. Catches what module
  units miss, like stdlib instances: 83.7% of per-function time against 76.2%
  here, and much more on a stdlib-heavy repository. Medium complexity: each
  function needs its own constant pool and program-wide helper names. In memory
  for one build, so nothing to invalidate. Release can't do this, because
  `default<O2>` works on a whole module.
- **Partition units by function identity.** Split each module's unit into
  hash buckets, or one unit per function. Simulated on the dumps:

  | Units | Objects a program | Dev time shared | Release time shared |
  |---|---:|---:|---:|
  | One per module, as today | 269 | 76.2% | 79.4% |
  | 16 buckets per module | 4,202 | 79.0% | 82.0% |
  | One per source declaration | 19,372 | 79.6% | 82.6% |
  | One per function | 27,918 | 83.3% | 83.6% |

  At most 7 more points, for 15 to 100 times the objects. Each cache file
  costs about 0.17 ms to write, and `lld` reads every object. Not worth it.
- **Compile every program in one program, linked many times.** Test batches
  already do this. It would also share monomorphize and the middle end, up to
  84% of their 2.3 s. But every artifact would depend on which other targets
  the build holds. `shares-counts`, task crossing and single-use inlining would
  become build-wide, and each link would read every program's code. Not for
  `buri build`.
- **A persistent cache per function.** It has the same key as the dev memo and
  only adds warm-build hits where a whole unit misses. It costs a file per
  function, like the identity partition. Not until the memo shows misses worth
  persisting.

**Recommended: make today's unit keys match first.** It's the biggest saving
and the smallest change. Then measure a stdlib-heavy neutral workspace before
building the dev memo.

### Landed

`closures::run` names a lifted lambda `{parent}$fn{n}`, with `n` counted
within its parent. The LLVM backend names a module by its unit alone, and
`codegen_key` and `backend::Options` no longer take the package path.

Cold `buri build //...` on the `full` workspace, `BURI_THREADS=1`, a
`validate`-profile `buri` with `--features backend-llvm`, CPU-seconds including
`lld`, best of two for dev:

| | Before | After | Saved |
|---|---:|---:|---:|
| Dev | 7.55 s | 6.44 s | 15% |
| Release | 291.6 s | 70.7 s | 76% |
| Units restored from the cache | 0 of 3,231 | 2,481 of 3,231 | 77% |

On the `ci` workspace, 88 of 178 units come back.
`cli/tests/build/codegen_sharing.rs` holds it by the profile's
`objects restored` count.
