# Compiler performance

**What the toolchain is expected to be fast at, how fast, and how that is
measured.** The audience is somebody about to optimize a phase — or about to
argue that a change is worth its complexity.

Three numbers, and everything else on this page exists to make them mean
something:

| Phase | Goal | Budget per line |
|---|---|---|
| Lexing **and** parsing | **10,000,000 lines/second** | 100 ns |
| Semantic analysis, type checking included | **1,000,000 lines/second** | 1 µs |
| Lowering to a binary or to JavaScript | **100,000 lines/second** | 10 µs |

They are **goals, not claims**. Nothing in the toolchain meets them today; §6
records by how much, and `cli/benches/compiler.rs` is what will keep saying so.

---

## 1. Why these three, and why these numbers

The three come from Chandler Carruth's *Modernizing Compiler Design for
Carbon's Toolchain* (CppNow 2023), which states the same ladder for the same
reason: a compiler's phases have wildly different costs per line, so one
aggregate figure hides which phase is the problem. Carbon's slides give the
budgets as 100 ns, 1 µs and 10 µs per line, and derive from the first of them
the constraints that shaped their whole front end — *200–300 cycles per line
lexed and parsed*, *about one main-memory access per line*, *no allocation per
token*.

Adopting the ladder rather than inventing one is deliberate. A goal nobody else
has tried to hit is a goal nobody can tell you is unreasonable, and this one has
public evidence on both sides: Carbon measured 6.7 M lines/s lexing and 1.9 M
lines/s through lex+parse on a server CPU, and Ben Titzer's public objection is
that 10 M lines/s is roughly 400 MB/s and that V8's JavaScript parser — the
fastest he knows of — runs at 60–80 MB/s. So the first goal is either at the
edge of what is possible or slightly past it, and that is the useful kind of
target: missing it by 3× is information, and missing it by 300× is a bug.

**Three separate budgets rather than one, and the first of them shared.**
Lexing and parsing are one budget because they are one decision: a front end
that fuses them, or that lexes lazily from the parser, or that parses straight
out of a token buffer, should be free to move work across that line without the
scorecard changing. They are still reported separately, because the split is
free — `parser::parse` calls `lex` as its first act — and because a regression
in one of them should not have to be inferred.

**Why the goals are per-phase throughput rather than end-to-end wall time.**
Wall time is what a user feels, and it is the wrong thing to hold a compiler to
in a design document: it moves with the build cache, with parallelism, with how
much of the standard library a program touches, and with the linker. Throughput
per phase is a property of the code in that phase, and it is the only figure
that says *which* phase to work on.

### What is deliberately not a goal here

- **End-to-end `buri build` time.** Governed by the action cache and by
  `design/native/BUILD-AND-WATCH.md`, not by this page. A toolchain that hit all
  three goals and rebuilt the world every time would still be slow.
- **Runtime performance of emitted code.** A different subject with a different
  measurement (`cli/tests/native/agreement.rs` and the JavaScript goldens).
- **Peak memory.** Worth a goal eventually; it does not have one yet, and this
  page should not pretend otherwise. Carbon measures it beside throughput and it
  is the obvious fourth column here.
- **Incremental re-analysis latency.** The language server's keystroke path is a
  latency question, not a throughput one, and a 100,000-line/second lowering
  phase is irrelevant to it.

---

## 2. What counts as a line

A benchmark whose denominator is undefined is a benchmark that can be argued
into any answer. So:

> **A line is a non-blank line of the input program's own source, comments
> included.**

Four decisions, each of which could have gone the other way:

1. **Non-blank.** A blank line is free in every phase, so counting them would
   make the toolchain look faster on prettier code. Carbon's generator emits 15%
   blank lines precisely because their absence would be unrepresentative; here
   they are generated *and* excluded from the denominator, which is the
   conservative combination.

2. **Comments count.** They are 22% of lines in the codebases Carbon measured
   and about the same here, the lexer reads every byte of them, and the parser
   attaches the doc comments among them to declarations. Excluding them would
   flatter the toolchain in exact proportion to how well the source is
   documented, which is the wrong incentive to build into a scorecard.

3. **The input program's lines, not the standard library's.** Every compilation
   also checks whatever of `core/*` it reaches, and at a thousand lines that
   fixed cost is most of the measurement. Counting those lines would make small
   programs look fast for a reason that has nothing to do with them. Instead the
   fixed cost is measured on its own and reported beside the rate — see §3, "The
   prelude floor".

4. **Lines, with bytes and tokens beside them.** The goal is stated in lines
   because that is the unit a person writes in. It is a *bad* unit for comparing
   two compilers or two languages — it moves with line density, and Buri's lines
   are shorter than C++'s — so the suite reports bytes/second and
   tokens/second in the same rows. Carbon reports all three for this reason, and
   the divergence between them is the signal: a line rate is hostage to source
   density, a byte rate to identifier length, and only the token rate tracks
   what the lexer's inner loop actually does.

### The rest of the protocol

- **In memory.** Sources are strings the benchmark already holds. No file is
  read inside a timer. `SourceMap` is populated before measurement.
- **Single-threaded.** The toolchain is single-threaded through the front end
  today. When that changes, the goals stay per-thread and a parallel figure is a
  new row, not a redefinition of these.
- **Release build.** `cargo bench` builds under `[profile.bench]`, which
  inherits `[profile.release]`: opt-level 3, LTO, one codegen unit. The
  `[profile.test] opt-level = 1` in the workspace manifest applies to test
  targets and does **not** reach a bench target. The benchmark prints which
  build it is, so a number taken from an unoptimized binary announces itself.
- **Warm up, then repeat.** At least 10 repetitions after a warmup, and at
  least three quarters of a second of sampling per row.
- **Median, with dispersion as MAD/median.** Not a mean, and not a standard
  deviation. A benchmark's distribution is one-sided — the machine can only make
  a run slower — so a symmetric summary is the wrong one and an
  outlier-sensitive one is worse. Carbon's own harness rejects normality
  outright and uses a non-parametric test for the same reason. The fastest
  sample is also reported, as the least-noise reading of the same quantity.
- **Frequency scaling and thermal drift are not controlled**, and this is a
  known weakness. The mitigations are the warmup, the median, and reporting
  dispersion so that a run taken on a throttled laptop is visibly noisier rather
  than quietly wrong. A ±MAD above about 5% should be treated as a run to
  discard rather than a number to record.

---

## 3. Benchmark validity

Everything below is a rule the suite follows. Where it comes from Carbon's
toolchain benchmarks, it says so; where the suite deliberately departs, §3.2
says why.

### 3.1 The rules adopted

**Generate most of the corpus; check a little of it in.** The rule used to be
"generate it, and check nothing in", and the argument was that a checked-in
megafile fixes one scale forever, drifts from the language as the language
moves, and cannot be reviewed. All three of those are still true, and none of
them is an argument against a *small* checked-in corpus with a manifest — they
are arguments against a large one with no provenance. What the old rule could
not do is compare a number taken today with one taken in March, because the
generator is a program under active development and a change to it moves the
bytes it emits without moving any code the benchmark measures.

So the suite runs two kinds of corpus, and each is answerable for something the
other cannot promise:

**Generated per run** — `cli/benches/generate.rs`, from a profile, a parameter
set and a fixed seed. This is what buys *scale flexibility* (1k to 100k on one
flag, and the 100k rows are 3.5 MB of source that has no business in a git
history), what buys *coverage of the parameter space* (twenty named profiles
cost nothing to keep), and what buys *no drift blindspot*: a generator that has
fallen out of the language shows up as a failed validation on the next run,
whereas a checked-in corpus can only fall out of the language silently and would
keep compiling long after the constructs in it stopped being idiomatic.

**Checked in** — `cli/benches/corpora/<name>/`, eight small corpora with a
`manifest.txt` recording the profile, the parameters, the seed, the generator
revision, the counts, and a digest. This is what buys *byte-stability over
time*, and that is the only thing it buys: two runs a year apart compile the
same bytes, so a difference between them is a difference in the compiler. It is
also what makes a change to the generator reviewable, because the diff of a
re-recorded corpus is the change's effect on the input, stated in the language
rather than in Rust.

Both kinds obey the same validity rules, without exception:

- **Both are compiled before either is measured.** A saved corpus that has
  stopped being valid Buri is a build failure, exactly as a drifted generator
  is. `--validate` covers both — whatever `--set` was asked for — and CI runs it.
- **Both are read into memory before any timer starts.** A saved corpus is
  loaded into the same `Program` a generator returns; the harness has one
  measurement path, and no file is read inside a timer.
- **Both must be reachable from `main`.** `--validate` reports the monomorphized
  function count for both, for the reason the next-but-one rule gives.
- **Both are stress-or-realistic, never both.** The family is a property of the
  profile and a saved corpus inherits it; the goal column is printed only for
  the realistic family, and `Family` is a type in `generate.rs` rather than a
  convention, so the rule is unrepresentable-to-violate rather than merely
  written down.
- **Neither is allowed to become the only one.** The headline scale — 100k lines
  — is generated and cannot be saved, and the saved anchor is 10k. So §6 records
  **both** the generated and the saved reading of `mixed`, and the two deltas are
  compared: when the compiler changes, both move together; when the *generator*
  changes, only the generated one moves. That pairing is what replaces the
  guarantee the old rule was reaching for, and it is stronger than either corpus
  alone.

And one rule that applies only to the saved half, because it is the failure mode
a checked-in corpus has and a generated one does not:

- **Regeneration is a break in the series, and it is announced.** A saved corpus
  is re-recorded only when it stops compiling or when the generator revision it
  names is retired; the re-record bumps `revision` in the manifest, `--json`
  carries `corpus_revision`, and §6 says which revision its numbers were taken
  at. A corpus that cannot be regenerated is deleted, not repaired.
  `cli/benches/corpora/README.md` is the operational form of this, with the caps
  — 512 KiB per corpus, 2 MiB in total — that keep the saved half small.

**Validate before measuring.** Every generated program is compiled through the
real front end — loader, checker, and all — and the suite exits non-zero if it
does not compile. *A benchmark over source that does not compile is a benchmark
of the error paths.* Carbon asserts `!buffer.has_errors()` inside each lexer
benchmark for exactly this; here the check is one level up, over the whole
corpus, before any timer starts.

**A realistic construct mix, not a single construct.** The shape the goals are
stated against emits what a real module contains: declarations with bodies,
structs and enums with derives, generic functions with and without bounds,
matches with guards and nested patterns, `?` chains, lambdas, string
interpolation, list literals, integer/hex/float/char/escaped-string literals,
and doc comments on most things. A file of nothing but `fn f(): Int { 1 }`
measures one path through the parser and almost nothing in the checker.

**Stress shapes, kept separate.** Fourteen of them (§4), each a single construct
pushed until it is the whole cost — deep expression nesting, one very wide
match, thousands of tiny functions, a handful of thousand-line functions, and
ten more that are the realistic mix with one weight turned up. They are *named
separately and never blended into the realistic mix*, because their purpose is
the opposite: to say which axis a phase is superlinear in. Carbon's
experience here is the argument for keeping both kinds: their realistic mix runs
at ~12 M tokens/s and their worst stress shape at under 100 k tokens/s, and a
suite with only one of the two would have reported a fiction.

**Many modules, not one file.** At 100,000 lines the mixed corpus is 389
modules with a real import graph, each module calling into one to three others'
functions *and* naming one of their types. Three reasons, all Carbon's: it stops
branch prediction from memorizing one file's shape, it gets closer to the
cache-cold behaviour that matters in practice, and it avoids anchoring on a
single file that may be unrepresentative. A fourth is specific to this
toolchain: cross-module resolution is where semantic analysis would be
superlinear if it were superlinear anywhere, and one big file would never
exercise it.

**Three orders of magnitude.** 1k, 10k, 100k lines. A single scale point cannot
show a cache cliff, and Carbon's numbers fall 6.70 → 5.02 M lines/s between 1k
and 256k — the fall-off *is* the finding.

**Phase isolation at the compiler's own seams.** Not a reimplementation of the
phases in the harness: each timer wraps the same function the driver calls. The
isolation falls out of the signatures — `Checker::run` takes `&Loaded` and
returns a fresh `Checked`, `monomorphize::run` takes `&Checked` and returns a
fresh `Program` — so a repetition cannot see the previous one's work, and
nothing has to be cloned to make that true. The parse cache
(`parser::Cache`) is filled before the semantic-analysis timer starts, which is
what keeps parsing out of that measurement.

**Block the optimizer, but not the code under test.** Each result goes through
`std::hint::black_box`. Carbon's sharper version of this — putting the barrier
on the loop's induction variable rather than the result, so the clobber does not
perturb codegen inside the region being measured, and making the loop index
data-dependent on the phase's return value to stop the CPU speculating into the
next iteration — is worth adopting if these rows ever get tight enough for it to
matter. It is not adopted yet; at the current gap factors it would be noise
about noise.

**Report the fixed cost separately.** See below.

#### The prelude floor

Semantic analysis of *any* program pays for the standard-library modules it
pulls in, whether the program is one line or a hundred thousand. The suite
measures that cost on its own — a module with the corpus's three imports and a
trivial `main` — prints it in the header, and reports both a gross rate and a
rate net of it. At 1,000 lines the floor is most of the measurement; at 100,000
it is a rounding error, and the two figures converging is itself a check that
the floor was measured correctly. Carbon has a whole second benchmark binary for
this (`prelude_benchmark.cpp`), and it is what explains their otherwise puzzling
result that checking is *faster* at 16k lines than at 256.

### 3.2 What was deliberately not adopted

- **`criterion`, or any benchmark framework.** The dependency bar in the
  workspace manifest admits code generators and platform interfaces; a
  statistics library is neither. What a harness has to do here is warm up,
  repeat, and report a median with its spread, and that is a hundred and fifty
  lines. The cost of this decision is real and worth naming: no bootstrapped
  confidence intervals, no automatic outlier classification, no HTML report.
  What replaces them is a `--json` mode and the discipline of reading the
  dispersion column.

- **Deterministic totals with randomized order.** Carbon's generator works
  hard to shuffle structure while holding the *total* count of every construct
  fixed, so that two runs do identical total work; a review found that not doing
  so cost 3% of run-to-run noise. This suite instead uses a *fixed seed*, so two
  runs compile byte-identical source and the totals are trivially equal. That is
  strictly stronger for run-to-run comparison and strictly weaker for one thing
  Carbon cared about: a fixed corpus can sit in a silent local minimum of the
  hash functions or the branch predictor. The trade is taken knowingly, and the
  escape hatch is that the seed is a constant one line from the top of
  `generate.rs`, reachable as `--seed=<hex>`, or a field in a saved corpus's
  manifest.

- **Re-randomizing per benchmark run so that ASLR noise shows up.** Same
  reason, same trade. What this suite reports is the spread of one binary's
  repetitions, not the spread across processes.

- **Hardware performance counters.** Carbon wires `libpfm` into google-benchmark
  and reports cycles and instructions, which is how a claim like "200–300 cycles
  per line" becomes checkable. There is no dependency-free way to do that here,
  and macOS has no `perf`. The substitute is §7's sampling profiler runs, which
  name the hot functions without quantifying them per byte.

- **Speed-of-light calibration benchmarks.** Carbon benchmarks `strcpy` and a
  tail-call byte-dispatch loop in the same binary, to bound what the hardware
  can do at all; without them "232 MB/s" is uninterpretable. This suite has no
  equivalent, and should get one before anybody works on the lexer — at the
  current gap the ceiling is not the binding constraint, but it will be.

- **A subprocess mode measuring end-to-end CLI time.** Carbon has both an
  in-process and a subprocess harness. Here that would measure the action cache,
  which is a different subject with a different design document.

- **Cross-compiler comparison.** Carbon's generator emits matched C++ so the
  same corpus can be run through Clang. There is no second Buri compiler.

---

## 4. The suite

Three files and a directory, no dependencies, one bench target:

| File | What it is |
|---|---|
| `cli/benches/generate.rs` | The source generator: the parameter space, the profiles, the seeded PRNG. |
| `cli/benches/corpus.rs` | Saved corpora: the manifest, the digest, discovery, `--record`, the size cap. |
| `cli/benches/compiler.rs` | The harness: warmup, repetition, median/MAD, the phase timers, the report. |
| `cli/benches/corpora/` | Eight checked-in corpora, 0.55 MB, capped at 2 MiB. |

`autobenches = false` in `cli/Cargo.toml` is what keeps the first two *modules*
of the `compiler` target rather than bench targets of their own: Cargo infers a
target from every `.rs` file directly under `benches/`, and two extra binaries
with no `main` is what a plain `cargo bench -p buri` would otherwise try to
build.

### Running it

```text
cargo bench -p buri --bench compiler                  # the table
cargo bench -p buri --bench compiler -- --quick       # 1k only, fewer reps
cargo bench -p buri --bench compiler -- --json        # one JSON document, for tracking
cargo bench -p buri --bench compiler -- --validate    # compile everything, measure nothing
cargo bench -p buri --bench compiler -- --split       # break lowering into its sub-phases
cargo bench -p buri --bench compiler -- --list        # the profile table, and the saved corpora
```

and the flags that select what runs:

```text
  --set=<name>      core | realistic | stress | native | saved | full   (default: core)
  --only=<text>     keep corpora whose label contains it
  --shape=<profile> one profile ad hoc, instead of a set
  --param <k>=<v>   override a dimension (repeatable; with --shape)
  --scale=<n>       target lines for --shape
  --seed=<hex>      seed for --shape
  --targets=<list>  js,macos-arm64,macos-x86_64,linux-x86_64,linux-arm64
  --record[=<name>] write the corpus into cli/benches/corpora/ and exit
```

`--validate` is the one to run in a hurry: it is what proves the corpus is still
valid Buri after a language change, and it is fast because it compiles each
program once instead of ten times. A generator that has drifted out of the
language shows up there as a list of diagnostics rather than as a benchmark
quietly measuring the error paths — and so does a *saved* corpus that has
stopped compiling, because `--validate` always covers the checked-in half
whatever `--set` was asked for. It also prints which backends this binary has,
which of the requested targets each can emit for, and how much of the 2 MiB
corpus budget is spent.

### The parameter space, and the profiles

A profile is a point in the generator's parameter space: `Params::default()`
with two or three fields moved. `Params` is about twenty dimensions — size and
distribution, a weight per construct kind, a size per construct, three surface
dials, and the reachability invariant — and `Params::default()` is
**byte-identical to the `mixed` corpus §6's numbers were taken over**, which is
a promise held by regenerating and diffing rather than by intention.
`--list` prints the profiles with their parameters, and
`--shape=<profile> --param k=v` runs a point that is not in the table. The table
is what the suite measures by default and what §6 reports; everything else is an
investigation, and an investigation that turns out to be worth watching becomes
a profile.

Two axes are deliberately *absent*. **Package structure**: the in-memory loader
is built with `Loader::new(None, ..)` and never consults a workspace, so
"many libraries" means "many clusters in the import graph" and the `clusters`
dimension says exactly that rather than implying more. **Parallelism**: the
front end is single-threaded, and §2 already covers it.

#### Realistic family

| Profile | Parameters moved | The question it answers |
|---|---|---|
| `mixed` | — | The headline. **Byte-identical to the corpus §6 quotes.** |
| `mixed-many-files` | `lines_per_module=40`, `fanout=2..5` | Many small files: per-module overhead, loader and symbol-table setup, and — natively — per-codegen-unit cost. |
| `mixed-few-files` | `lines_per_module=5000`, `fanout=1..2` | Large files: whether anything is superlinear in module *size* rather than count. |
| `mixed-libs` | `clusters=12`, `cross_cluster=8` | Many libraries: a clustered import graph with thin edges between clusters. |
| `mixed-deep-graph` | `dep_span_pct=5` | A deep dependency chain rather than a wide fan: transitive resolution depth. |
| `mixed-wide-graph` | `fanout=6..12` | Import-graph fan-out at the same line count. |

#### Stress family

| Profile | Parameters moved, or its own emitter | The question |
|---|---|---|
| `deep-nesting` | own emitter | Recursive descent; the parser's depth guard. |
| `wide-match` | own emitter | A quadratic term in exhaustiveness or decision-tree construction. |
| `many-small-fns` | own emitter | Per-item overhead. |
| `few-large-fns` | own emitter | Per-body cost. |
| `struct-heavy` | `w_struct=8`, others 0 but `w_arith_fn`; `fields_per_struct=6..12` | Layout, derives, field resolution. |
| `struct-light` | `w_struct=0` | The control for the row above. Only meaningful as a pair. |
| `enum-heavy` | `w_enum=8`, `variants_per_enum=12..24` | Enums matched exhaustively — the *realistic* neighbourhood of `wide-match`. |
| `generic-blowup` | `w_generic_fn=8`, `generic_args=8` | Monomorphization: eight copies of every generic, at one source size. |
| `derive-heavy` | `derives=6`, `w_struct=4`, `w_enum=4` | `middle::derives`, which only the native branch runs — invisible in every JS row. |
| `impl-heavy` | `methods_per_struct=12`, `w_struct=6` | Method resolution and per-impl setup. |
| `match-heavy` | `w_match_fn=8`, `match_arms=8..20` | Decision-tree construction on realistic arm counts. |
| `comment-heavy` | `comment_block_lines=6` | The lexer's comment path, and the honesty of §2's "comments count". |
| `comment-free` | `doc_comment_pct=0` | The control. The *ratio* of the two is the number worth recording. |
| `long-idents` | `ident_len=32` | Bytes/token and the lexer's identifier path, at a fixed token count. |

The dimensions, for `--param`: `lines`, `lines_per_module`, `clusters`,
`cross_cluster`, `fanout`, `dep_span_pct`, `w_struct`, `w_enum`,
`w_generic_fn`, `w_arith_fn`, `w_match_fn`, `w_string_fn`, `w_list_fn`,
`fields_per_struct`, `variants_per_enum`, `methods_per_struct`, `derives`,
`body_lets`, `match_arms`, `generic_args`, `nesting`, `doc_comment_pct`,
`comment_block_lines`, `blank_pct`, `ident_len`, `reach`, `seed`. Values are
decimal integers, `true`/`false`, `lo..hi` inclusive ranges, or `0x…`.

### The phase seams

```text
lex              parsing::lexer::lex                       text     -> tokens
lex+parse        parsing::parser::parse                    text     -> tree          (goal 1)
sema             semantics::resolve::Checker::run          Loaded   -> Checked       (goal 2)
lower+js         monomorphize::run + actions::emit         Checked  -> JavaScript    (goal 3)
lower+<triple>   monomorphize::run + actions::prepare
                                   + Backend::emit         Checked  -> object bytes  (goal 3)
```

`actions::emit` is the same call `buri build --output=js` makes, `prepare` — and
therefore `middle::run` — included.

The native rows go through the same two calls `actions::objects_of` makes below
the front end — `prepare`, which is the one place the middle-end pipeline is
chosen, and `Backend::emit` — and stop there. **Nothing is linked and nothing is
run**: the link is the only host-only step, because the runtime archive
`cli/build.rs` embeds is built for the host and for nothing else, and goal 3 is
stated over lowering rather than over producing an executable. The second
`lower::run` that `objects_of` performs for the cache keys is excluded too: that
is the build system paying for content-addressing, not the compiler lowering the
program.

Each repetition rebuilds the monomorphized program, because `prepare` mutates in
place and is not idempotent. Monomorphization is therefore inside every lowering
row, JavaScript and native alike, which is what keeps the three comparable;
`--split` subtracts it and reports
`mono | middle-A | middle-native | lower(IR) | emit`, to stderr, so
`--json --split` still emits one parseable document.

Three native triples by default — `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` — whichever machine
the suite is run on. Cranelift is compiled with `all-arch`, so selecting an ISA
by triple costs nothing, and a cross triple is *more* reproducible than the host
one: the host ISA is inferred from the running CPU's features and a cross ISA is
the baseline for its triple. The refusal to cross-compile stays where it belongs,
at `link::can_link` and `actions::native_ready`, which is what
`buri build --output=linux/x86_64` on a mac still answers to; `cranelift::isa_for`
no longer states the same policy a second time.

`Profile::Debug` selects Cranelift and `Profile::Release` selects LLVM, so a
release row is an LLVM row and it is taken only on a toolchain built with
`backend-llvm`. **There is no `#[cfg]` anywhere in the harness**: it asks
`backend::select` and prints `skipped: <diagnostic>` rather than testing a
feature, so the report always says which rows *this* binary could not take
rather than changing shape depending on how it was compiled. Skipped rows go in
their own `skipped` array in `--json` rather than into `rows`, so a consumer of
the row schema is unaffected.

`Backend::missing_intrinsics` is asked before any timer, for the same reason the
corpus is compiled before any timer: a backend that would have failed must not
be measured failing. Two honest reasons a native row is skipped today, both
found by the first run of these rows and both worth watching:

- **`list.filter`, `list.fold`, `list.mapCtx`.** The Cranelift backend has no
  body for them, and the realistic mix calls all three, so every realistic
  native row skips. `--set=native` therefore also carries `struct-heavy`,
  `enum-heavy`, `wide-match` and `deep-nesting`, which are the profiles the
  backend can currently take.
- **`Too many return values to fit in registers`** on either x86_64 triple, for
  `main`: `Result<(), Str>` is three scalars and the SysV return convention has
  two. It fails identically for `macos-x86_64`, so it is an architecture fact
  and not a cross-compilation one — which is why `linux-arm64` is a default
  target, so that the report distinguishes "cross codegen does not work" from
  "the return ABI does not".

---

## 5. Where the corpus stood when this was written

The before-picture, so that "the compiler got smaller" is a claim somebody can
check. Rust under `cli/src`, counted by a script rather than by `tokei`, which
is not installed:

| Area | Files | Code | Comment | Blank | Total | Comment share |
|---|---:|---:|---:|---:|---:|---:|
| `parsing` (lexer, parser, tree) | 4 | 3,693 | 568 | 276 | 4,537 | 13% |
| `compiler/semantics` | 9 | 6,982 | 1,121 | 455 | 8,558 | 14% |
| `compiler/middle` | 12 | 10,860 | 3,157 | 887 | 14,904 | 23% |
| `compiler/backend/js` | 4 | 5,343 | 1,178 | 328 | 6,849 | 18% |
| `compiler/backend/cranelift` | 5 | 3,865 | 1,360 | 274 | 5,499 | 26% |
| `compiler/backend/llvm` | 6 | 5,707 | 2,361 | 348 | 8,416 | 29% |
| `build` | 12 | 7,488 | 1,794 | 601 | 9,883 | 19% |
| `commands` | 12 | 3,537 | 889 | 226 | 4,652 | 20% |
| `documentation` | 9 | 5,063 | 1,054 | 443 | 6,560 | 17% |
| `language_server` | 4 | 1,048 | 265 | 114 | 1,427 | 20% |
| shared, driver, stdlib glue | 10 | 4,506 | 1,441 | 446 | 6,393 | 24% |
| **`cli/src` total** | **87** | **58,092** | **15,188** | **4,398** | **77,678** | **21%** |
| `cli/tests` | 29 | 10,751 | 3,695 | 1,098 | 15,544 | 26% |
| `cli/benches` | 2 | 1,047 | 293 | 100 | 1,440 | 22% |

And the Buri-language side, which the compiler has to get through:

| | Files | Code | Comment | Blank | Total |
|---|---:|---:|---:|---:|---:|
| standard library (`core/*`) | 31 | 2,925 | 1,564 | 623 | 5,112 |
| test corpus (`cli/tests/**/*.buri`) | 854 | 15,334 | 2,000 | 2,861 | 20,195 |
| shipped documentation (`.md`) | 107 | — | — | — | 11,213 |

The three phases the goals name are **28,000 lines of Rust** between them:
`parsing` at 4.5k, `semantics` at 8.6k, and `middle` plus `backend/js` at 21.8k.
That is the surface any optimization wave has to work on.

---

## 6. Where the toolchain stands

Two snapshots, both on an M-series MacBook (macOS, aarch64, 10 cores), release
build, seed `0x0b001a575eed0001`, protocol as §2. A gap of 1.0 means the goal
is met; below 1.0 means it is beaten.

### 6.1 The baseline, 2026-08-17, before any optimization

| Corpus | Phase | Lines/s | Gap | ±MAD |
|---|---|---:|---:|---:|
| mixed/100k | lex | 5.51 M | 1.8× | 1.1% |
| mixed/100k | lex+parse | 1.45 M | **6.9×** | 0.9% |
| mixed/100k | sema | 830 k | 1.2× | 1.0% |
| mixed/100k | lower+js | 237 k | 0.4× | 1.3% |
| wide-match/10k | sema | **56 k** | **17.8×** | 0.7% |
| many-small-fns/10k | lower+js | **17 k** | 6.0× | 1.8% |
| few-large-fns/10k | lower+js | **7 k** | 14.0× | 0.8% |

What it said: the parser was where the first goal lived or died (~510 ns/line
against a whole-budget of 100); sema was a constant factor off on realistic
code and superlinear on one stress shape; lowering beat its goal on `mixed`
and collapsed on both function-shape extremes.

### 6.2 After optimization waves 1–2, same day

Wave 1: parser call-site cleanup (a cloned 128-byte token returned per `bump`,
93 sites), a Pratt loop replacing the ten-function precedence ladder, pre-sized
token buffers; six quadratic terms removed from exhaustiveness checking and
variant lookup; ten mechanical clone eliminations in the checker. Wave 2:
trivia and numeric raw text moved off the token to side tables (`Token` 112 →
48 B, `Tok` 48 → 32 B, `Expr` 112 → 64 B, `Item` 272 → 16 B, pinned by
`size_of` asserts); one Θ(body²) term (`resolve_map`) and a
Θ(fns × globals²) term (release-mode mangling) removed from JS lowering.
All 683 tests unchanged and green; emitted JavaScript verified byte-identical.

| Corpus | Phase | Lines/s | vs baseline | Gap | ±MAD |
|---|---|---:|---:|---:|---:|
| mixed/100k | lex | 6.19 M | 1.12× | 1.6× | 0.8% |
| mixed/100k | lex+parse | 2.94 M | **2.03×** | **3.4×** | 0.7% |
| mixed/100k | sema | 999 k | 1.20× | **1.0×** | 1.6% |
| mixed/100k | lower+js | 273 k | 1.15× | 0.4× | 1.6% |
| wide-match/10k | sema | 1.29 M | **23×** | 0.8× | 1.9% |
| many-small-fns/10k | lower+js | 206 k | **12×** | 0.5× | 2.4% |
| few-large-fns/10k | lower+js | 356 k | **50×** | 0.3× | 0.8% |

Both tables are the *generated* `mixed` corpus at generator revision 1, which is
byte-identical to `cli/benches/corpora/mixed-10k` at the 10k scale — the pairing
§3.1 requires. The native rows, the fourteen stress profiles and the saved rows
arrive with the next measurement; these tables predate them and say so rather
than being back-filled with numbers nobody took.

### The reading, after waves 1–2

1. **Goals two and three are met.** Sema crosses 1 M lines/s on `mixed` at
   every scale (1.19 M at 10k, 999 k at 100k — the 100k row sits on the line
   and should be watched, not celebrated), and every former sema/lowering
   collapse now beats its goal. The one shape still short anywhere is
   `many-small-fns` sema at 1.3×, which is per-item overhead, not an
   algorithm.

2. **Goal one is at 3.4× and is now purely a representation problem.** The
   front end still does ~0.9 allocator round-trips per token, and the
   remaining plan of record is Carbon-shaped: SoA token storage, a flattened
   index-based AST (no `Box` per node), and interned symbols. The wave-2
   estimate for that stack is 7–9 M lines/s; §3.2's missing speed-of-light
   calibration should land before the last factor is chased.

3. **What was tried and rejected**, so it is not tried twice: an inline
   small-string for identifiers (+9% parse, −7–11% sema — net loss);
   identifier interning at lex time (adds probes to the lean phase to pay a
   phase already at goal); a custom Swiss table (hashbrown already is one, and
   no hash table sits on the lex/parse hot path at all — measured zero hash
   self-samples); a state-machine parser (merges five well-predicted dispatch
   sites into one megamorphic branch); checking-by-lowering to a semantic IR
   (Carbon's own SemIR checker measures ~0.5–0.7 M lines/s, slower than this
   checker).

The prelude floor at the baseline: lex 1.7 µs, parse 5.1 µs, sema 0.52 ms,
lower 1.08 ms. At 1k lines the sema floor is a third of the measurement — the
gross/net divergence at that scale is the floor being correctly subtracted,
and the two figures converging by 100k is the §3 self-check passing.

### 6.3 After the flattened tree and the native unblocking, 2026-08-18

The representation wave: the parse tree now lives in append-only arenas
(24-byte nodes, parallel span arrays, children as contiguous ranges), names
are source spans, tokens borrow the source instead of owning `String`s, every
consumer — checker, formatter, docs, lint, LSP — reads `Copy` views over
indices, and the twelve owned node types are deleted (`tree.rs` lost 353
lines). Front-end allocations fell from ~6,100 to ~3,000 per 1,000 lines, and
the lexer's own from 2,611 to 512. Beside it, the Cranelift backend gained the
nine open-coded list loops and an x86_64-correct return ABI (wide results
through a trailing out-pointer), which took the native lowering rows from
"skipped" to measured on all three triples — and fixed a one-block-per-element
leak on `[Str]` while it was there. All 685 pinned tests pass; formatting,
diagnostics, and emitted output are byte-identical throughout.

The calibration rows (§3.2's debt, now paid) put this machine's
speed-of-light at ~76 M lines/s for the raw byte-scan + token-write +
node-write streams, so the 10 M goal is physics-approved and the remaining
gap is engineering.

| Corpus | Phase | Lines/s | Gap | ±MAD |
|---|---|---:|---:|---:|
| mixed/100k | lex | 9.28 M | 1.08× | 0.7% |
| mixed/100k | lex+parse | **4.09 M** | **2.4×** | 1.8% |
| mixed/100k | sema | 989 k | 1.0× | 1.3% |
| mixed/100k | lower+js | 285 k | 0.35× | 0.9% |
| mixed/100k | lower+macos-arm64 | 55 k | 1.8× | 1.3% |
| mixed/100k | lower+linux-x86_64 | 53 k | 1.9× | 1.5% |
| mixed/100k | lower+linux-arm64 | 54 k | 1.9× | 0.9% |

Against the baseline: lex+parse 1.45 M → 4.09 M (2.8×), lex 5.5 M → 9.3 M,
and the parse half alone is ~4.4× faster. The `saved:` corpora agree with the
generated ones within noise, which is the §3.1 dual-scheme check passing.

What the expanded matrix says is left, in the order it hurts:

1. **Goal one at 2.4×.** The remaining allocations are the owned `Ident` and
   `TypeExpr` at the declaration level (~1,000/1k lines, pinned by
   `cli/tests/language/standard_library.rs:69-73` — relaxing that pin is a
   product decision, not an optimization), and the deferred SoA token
   storage. `comment-free` (4.7×) and `many-small-fns` (3.7×) remain the
   honest rows: declaration-dense source pays the declaration-level residue
   hardest.

2. **Native lowering is 1.7–1.9× short on realistic code** — a constant
   factor, uniform across triples, so it is codegen cost, not a
   cross-compile artifact. Two shapes are worse and both are algorithmic:
   `enum-heavy` at 7.1–7.4×, and `wide-match` falling 109 k → 16 k between
   1k and 10k, a superlinear term the JS backend does not share (its
   `wide-match` row sits at 0.95×).

3. **Sema holds at goal** on `mixed` at every scale, `enum-heavy`, and the
   saved corpora; `many-small-fns` (1.28×) and `comment-free` (1.12×) are
   the per-declaration overhang, same residue as goal one's.

### 6.4 Round two: SoA tokens and the native quadratics, 2026-08-18

Two waves. The token stream became columns — a dense kind byte, a span
column, a payload column, sparse side tables; 13 bytes/token against 48 —
with the lexer's write path split into an inlined three-store fast path and a
cold trivia attach. And the native path lost three quadratic terms
(`Layouts::of` deep-copying per operand, `decision::group` rescanning arms
per head, `rc::Scan::child` walking sibling chains) plus a 100-block RC
inline replaced by a call to the per-type glue the backend already emits.
All 685 tests unchanged; JS goldens byte-identical.

| Corpus | Phase | Lines/s | Gap | Movement |
|---|---|---:|---:|---|
| mixed/100k | lex | **11.2 M** | **0.90× — MET** | 5.5 M at baseline |
| mixed/100k | lex+parse | 4.71 M | 2.1× | 1.45 M at baseline |
| mixed/100k | sema | 968 k | 1.03× | hovers on the line, run to run |
| mixed/100k | lower+js | 278 k | 0.36× | met since baseline |
| mixed/100k | lower+native ×3 | 51–55 k | 1.8–2.0× | unmoved — see below |
| wide-match/10k | lower+native | 129 k | **0.77× — MET** | was 15 k, cliff gone |
| enum-heavy/10k | lower+native ×3 | 20–21 k | 4.7–5.1× | was 7.4× |

Where the remaining gaps actually live, measured rather than suspected:

- **lex+parse (2.1×):** half the remaining allocations are the owned
  `Ident`/`TypeExpr` shape at the declaration level, pinned by
  `cli/tests/language/standard_library.rs:69-73` — measured at 11.2% of the
  phase, provably unavoidable while the pin stands. The plateau without
  moving it is ~5.5–6 M; 10 M additionally needs the C3 rewrite. Both are
  product decisions now, not optimizations.
- **native realistic (1.8–2.0×):** 88% of the row is Cranelift's own
  `define_function`, 42% regalloc2 alone. The lowering this repository owns
  is no longer the cost; the next step would be a value-model change or a
  different codegen strategy, not a faster loop.
- **enum-heavy native (4.7×):** per-type RC glue and derive volume; the
  8.4× term is gone and what remains scales linearly.
- Measured dead ends, recorded so they stay dead: branchless RC (+15%),
  `opt_level = "speed"` (+20%), `regalloc_algorithm = "single_pass"`
  (silently inert since Cranelift 0.123).

---

## 7. Profiling, on this platform

There is no `perf` on macOS and no hardware-counter dependency in the tree
(§3.2), so the way to turn a §6 gap into a function name is a sampling
profiler over the bench binary:

```text
cargo bench -p buri --bench compiler --no-run     # build it, find the binary path
samply record <bench-binary> --quick              # if samply is installed
xcrun xctrace record --template 'Time Profiler' --launch <bench-binary> -- --quick
```

`--quick` keeps the run short enough to profile; the phase timers dominate the
samples, so the hot functions under `lex`, `parse`, `Checker::run` and the
lowering calls are directly attributable to their rows. What a profile cannot
give here is cycles-per-line; the substitute discipline is to re-run the suite
after every change and let §6's table, not the profile, say whether the change
was real.

