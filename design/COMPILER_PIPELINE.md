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
  place, and again per program in `Checked::restyle`. 3.0%, of which styles are
  1.1%.
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

Check passed its goal when §6.97 stopped checking each library once per
program. Dev codegen is short because it runs once per program. Each program
reaches 104k to 166k lines, so the back half compiles about 1.65 M lines for
262k unique ones. Measured per program reached, `cmd/p11` monomorphizes,
runs the middle end and emits at 470k lines a CPU-second.

**Most per-program work is repeated.** The 12 linked executables define
203,858 functions between them, and only 63,313 distinct symbols (`nm -U`).
5,566 symbols appear in all 12. Not one of the 3,231 codegen-unit keys repeats
across programs (`--explain`), so the cache and `Flights` share nothing.

## Proposals

Ranked by CPU saved per unit of complexity. Savings are of the cold builds
above.

1. **Make codegen units the same across programs.** Up to 39% of dev CPU and
   68% of release. Evidence: 69% of linked functions are another program's by
   symbol; the back half below check is 4.1 s of dev and 290 s of release.
   That's an upper bound. A matching symbol isn't a matching body, because
   `inline` pastes single-call-site callees by program-wide counts and closure
   numbers move. §6.89 found 18% by symbol and 42% by body on a repository
   generic over its handlers. First step, cheap: hash each function's
   `render_func_into` text across programs to get this workspace's
   body-level share. Then split each module's unit by whether a function's
   body depends on the program, and keep program-driven inlining out of the
   shared half. Risk: changes symbols and binaries, and can lose inlining
   across the split. It removes the per-program repetition the goals
   denominator hides; it adds a partition rule.
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
