# Compiler performance

**What the toolchain should be fast at, how fast, and how the suite measures
it.** Written for somebody about to optimize a phase, or about to argue that a
change is worth its complexity.

Three numbers, and everything else on this page exists to make them mean
something:

| Phase | Goal | Budget per line |
|---|---|---|
| Lexing **and** parsing | **10,000,000 lines/second** | 100 ns |
| Semantic analysis, type checking included | **1,000,000 lines/second** | 1 µs |
| Lowering to a binary or to JavaScript | **100,000 lines/second** | 10 µs |

They are **goals, not claims**. Semantic analysis and both lowering paths meet
theirs, the native one since 2026-08-29, its first time. Lex+parse does not, and
§6 records by how much: 1.34× short on a loaded machine on 2026-10-03, after
§6.14 took it 1.56× faster. `cli/benches/compiler.rs` is what keeps saying so.

---

## 1. Why these three, and why these numbers

The three come from Chandler Carruth's *Modernizing Compiler Design for
Carbon's Toolchain* (CppNow 2023), which gives the same ladder for the same
reason: a compiler's phases cost wildly different amounts per line, so one
aggregate figure hides which phase is the problem. Carbon's budgets are the same
100 ns, 1 µs and 10 µs, and the first gave them the constraints that shaped
their whole front end — *200–300 cycles per line lexed and parsed*, *about one
main-memory access per line*, *no allocation per token*.

Borrowing the ladder rather than inventing one buys public evidence on both
sides. Carbon measured 6.7 M lines/s lexing and 1.9 M through lex+parse on a
server CPU. Ben Titzer objects in public that 10 M lines/s is roughly 400 MB/s,
and that V8's JavaScript parser — the fastest he knows of — runs at 60–80 MB/s.
So the first goal sits at the edge of what is possible, which is the useful kind
of target: missing it by 3× is information, missing it by 300× is a bug.

**Lexing and parsing share a budget** because they are one decision: a front end
that fuses them, or lexes lazily from the parser, should be free to move work
across that line without the scorecard changing. The suite still reports them
separately, so you do not have to infer a regression in one of them.

**The goals are per-phase throughput rather than end-to-end wall time.** Wall
time moves with the build cache, with parallelism, with how much of the standard
library a program touches, and with the linker. Throughput per phase is a
property of the code in that phase, and it is the only figure that says *which*
phase to work on.

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

A benchmark with an undefined denominator can be argued into any answer. So:

> **A line is a non-blank line of the input program's own source, comments
> included.**

Four decisions, each of which could have gone the other way:

1. **Non-blank.** A blank line is free in every phase, so counting them would
   make the toolchain look faster on prettier code. The generator emits 15% of
   them, as Carbon's does, *and* drops them from the denominator.

2. **Comments count.** They are 22% of lines in the codebases Carbon measured
   and about the same here, the lexer reads every byte of them, and the parser
   attaches the doc comments among them to declarations. Dropping them would
   flatter the toolchain in proportion to how well the source is documented.

3. **The input program's lines, not the standard library's.** Every compilation
   also checks whatever of `core/*` it reaches, and at a thousand lines that
   fixed cost is most of the measurement. The suite measures it on its own and
   reports it beside the rate — see §3, "The prelude floor".

4. **Lines, with bytes and tokens beside them.** The goals use lines because
   that is the unit a person writes in. Lines are a *bad* unit for comparing two
   compilers — they move with line density — so the suite reports bytes/second
   and tokens/second in the same rows. Where the three diverge is the signal: a
   line rate is hostage to source density, a byte rate to identifier length, and
   only the token rate tracks what the lexer's inner loop does.

### The rest of the protocol

- **In memory.** Sources are strings the benchmark already holds. No timer ever
  spans a file read, and the benchmark fills the `SourceMap` before it measures.
- **Single-threaded.** The toolchain is single-threaded through the front end
  today. When that changes, the goals stay per-thread and a parallel figure is a
  new row, not a redefinition of these.
- **Release build.** `cargo bench` builds under `[profile.bench]`, which
  inherits `[profile.release]`: opt-level 3, LTO, one codegen unit. The
  `[profile.test] opt-level = 1` in the workspace manifest does **not** reach a
  bench target. The benchmark prints which build it is, so a number taken from
  an unoptimized binary announces itself.
- **Warm up, then repeat.** At least 10 repetitions after a warmup, and at
  least three quarters of a second of sampling per row.
- **One documented deviation, above 500,000 lines.** The scale tier (§4) takes
  at least **3** repetitions rather than 10, and one warmup call rather than
  two. Nothing else changes, including the three-quarter-second sampling floor,
  so the cheap phases still take nine or ten repetitions and only the expensive
  ones fall to three. The reason is arithmetic: native lowering at a million
  lines is thirty seconds a repetition, so ten repetitions would cost six
  minutes for one row and about forty for the tier. Rows taken under the
  deviation say so, in the table and in `--json`'s `protocol` field. It costs
  the dispersion column — a MAD over three samples is a much weaker statement
  than a MAD over ten — so read the scale rows for their order of magnitude
  rather than their last digit.
- **Median, with dispersion as MAD/median.** A benchmark's distribution is
  one-sided — the machine can only make a run slower — so a symmetric summary is
  the wrong one and an outlier-sensitive one is worse. The suite also reports
  the fastest sample, as the least-noise reading of the same quantity.
- **Nothing controls frequency scaling or thermal drift**, and that is a known
  weakness. The warmup, the median, and the reported dispersion are the
  mitigations: a run taken on a throttled laptop comes out visibly noisier
  rather than quietly wrong. Treat a ±MAD above about 5% as a run to discard
  rather than a number to record.

---

## 3. Benchmark validity

Everything below is a rule the suite follows. Where it comes from Carbon's
toolchain benchmarks, it says so; where the suite deliberately departs, §3.2
says why.

### 3.1 The rules adopted

**Generate most of the corpus; check a little of it in.** A large checked-in
corpus fixes one scale forever, drifts from the language as the language moves,
and cannot be reviewed. A generated one cannot compare a number taken today with
one taken in March, because the generator is under active development and a
change to it moves the bytes it emits without moving any code the benchmark
measures. So the suite runs three kinds of corpus, each answerable for something
the others cannot promise:

**Generated per run** — `cli/benches/generate.rs`, from a profile, a parameter
set and a fixed seed. This buys *scale flexibility* (1k to 100k on one flag, and
the 100k rows are 3.5 MB of source that has no business in a git history),
*coverage of the parameter space* (twenty named profiles cost nothing to keep),
and *no drift blindspot*: a generator that has fallen out of the language fails
validation on the next run, while a checked-in corpus falls out of the language
silently and keeps compiling long after its constructs stop being idiomatic.

**Checked in** — `cli/benches/corpora/<name>/`, eight small corpora with a
`manifest.txt` recording the profile, the parameters, the seed, the generator
revision, the counts, and a digest. This buys *byte-stability over time*: two
runs a year apart compile the same bytes, so a difference between them is a
difference in the compiler. It also makes a change to the generator reviewable,
because the diff of a re-recorded corpus shows the change's effect on the input,
in Buri rather than in Rust.

**Digest-pinned** — `cli/benches/pinned/<name>.txt`, a manifest with no source
beside it: what a saved corpus's manifest records, plus the SHA-256 of the bytes
that combination produced. The harness regenerates the corpus on every run and
checks the digest **before it measures anything**. This buys byte-stability *at
a scale a git history cannot hold*: the 100,000-line corpus is 3.5 MB and the
million-line one 35 MB, against a repository whose whole history is 15 MB, and
the manifest for either is four hundred bytes.

There are forty of them: **twenty parameter points at two scales**. A point is a
name, a seed and a delta from `Params::default()`, and its two corpora share
that seed, so the only difference between a point's 100k row and its 1M row is
the size. The whole set costs 15,546 bytes of git — `cat
cli/benches/pinned/*.txt | wc -c`, 2026-09-01. §4 lists the points and what each
moves.

A saved corpus bundles two separable properties: "these are the bytes" is worth
checking in, and "here they are" is what costs the megabytes. A digest gives the
first without the second, and the check is the same one — `corpus::digest` is
one function and both kinds go through it, so pinning a corpus that is *also*
checked in produces the same hash.

It gives up the reviewable diff, and that is the whole cost. When a saved corpus
moves, the diff says *what* moved, in Buri. When a pinned one moves, the failure
carries two hashes and the counts beside them — which is why the manifest
records `lines`, `bytes` and `modules` as well, so a mismatch can say whether
the shape changed or only its contents. Recovering the rest means regenerating
both revisions by hand. That trade is right for the scale tier and wrong for a
1,000-line corpus, which is why both kinds exist.

All three kinds obey the same validity rules, without exception:

- **The suite compiles all of them before it measures any.** A saved corpus that
  has stopped being valid Buri is a build failure, exactly as a drifted
  generator is. `--validate` covers the saved half whatever `--set` you asked
  for, and CI runs it. How much of the *pinned* half it covers is `--set`'s
  business, because regenerating and digesting forty corpora takes three
  minutes: none under `--quick`, the CI gate, at 0.4 s; the anchor at both
  scales under a plain `--validate`, thirteen seconds; the sample under
  `--validate --set=scale`, twenty-seven; all forty under
  `--validate --set=scale-full`, four minutes and twenty (§4). The rule that
  does not bend is the last row: **one documented command checks the whole
  pinned half**, and a re-pin is what happens when it fails.
- **All are in memory before any timer starts.** The harness loads a saved
  corpus, and regenerates *and* digest-checks a pinned one, into the same
  `Program` a generator returns. One measurement path, and no timer spans a file
  read.
- **All must be reachable from `main`.** `--validate` reports the monomorphized
  function count for each.
- **Every corpus is stress or realistic, never both.** The family belongs to the
  profile, and a saved or pinned corpus inherits it. The harness prints the goal
  column only for the realistic family, and `Family` is a type in `generate.rs`
  rather than a convention, so violating the rule is unrepresentable. One
  derivation sits on top: a corpus whose `params` move anything its profile does
  not **is** a stress shape, whatever family its base profile belongs to, so
  `mixed` with `w_string_fn=8` is quoted against no goal. The manifest says so.
- **No corpus is allowed to become the only one.** The headline scale — 100k
  lines — is generated *and* pinned, and the saved anchor is 10k. So §6 records
  **both** the generated and the saved reading of `mixed`, and compares the two
  deltas: when the compiler changes, both move together; when the *generator*
  changes, only the generated one moves. The pinned 100k row is the third leg —
  the same bytes as the generated 100k row, checked — so the two agreeing is the
  pinning scheme reporting that it works.

And two rules for the kinds with a manifest, because it is the failure mode a
recorded corpus has and a generated one does not:

- **Re-recording breaks the series, so it gets announced.** A saved corpus gets
  re-recorded, and a pinned one re-pinned, only when it stops compiling or when
  the generator revision it names is retired. That bumps `revision` in the
  manifest, `--json` carries `corpus_revision`, and §6 says which revision its
  numbers came from. A corpus that cannot be regenerated gets deleted, not
  repaired. `cli/benches/corpora/README.md` and `cli/benches/pinned/README.md`
  are the operational form of this, with the caps — 512 KiB per corpus, 2 MiB in
  total — that keep the saved half small. The pinned half has no cap because it
  has no size: it is a manifest.
- **A pinned digest that does not match stops the run**, not a warning. A
  corpus that has drifted out of the language announces itself by failing to
  compile; one that has drifted *within* the language announces itself here and
  nowhere else. The failure prints both digests and both sets of counts, and the
  fix is either to find what moved in the generator or to re-pin deliberately.

**Validate before measuring.** The suite compiles every generated program
through the real front end and exits non-zero if it does not compile. *A
benchmark over source that does not compile is a benchmark of the error paths.*

**A cell that drives the binary starts from a cache no other binary wrote.**
Not the bench target, which compiles in process and keeps no cache — the
end-to-end cells §6 quotes. A cache key carries `arguments::VERSION`, which is
`CARGO_PKG_VERSION`: a *version*, not a hash of the running executable. So
rebuilding `buri` at the same version moves no key, and the first build in a
workspace whose `.buri` a previous binary wrote mixes the two compilers'
objects. Only that first build is affected, which is what makes the reading look
like noise. Use a fresh tree, or `--force`, or `buri clean`, before a cell that
spans a compiler rebuild. `buri docs build/hermeticity`, "The toolchain in the
key", is the same warning where a user will find it.

**A realistic construct mix, not a single construct.** The shape the goals are
stated against emits what a real module contains: declarations with bodies,
structs and enums with derives, generic functions with and without bounds,
matches with guards and nested patterns, `?` chains, lambdas, string
interpolation, list literals, integer/hex/float/char/escaped-string literals,
and doc comments on most things. A file of nothing but `fn f(): Int { 1 }`
measures one path through the parser and almost nothing in the checker.

**Stress shapes, kept separate.** Fourteen of them (§4), each a single construct
pushed until it is the whole cost. They are *named separately and never blended
into the realistic mix*, because their purpose is the opposite: to say which
axis a phase is superlinear in. Carbon's realistic mix runs at ~12 M tokens/s
and their worst stress shape at under 100 k tokens/s, so a suite with only one
of the two would have reported a fiction.

**Many modules, not one file.** At 100,000 lines the mixed corpus is 348
modules with a real import graph — `--validate`'s own count at generator
revision 7 — each module calling into one to three others' functions *and*
naming one of their types. Three reasons, all Carbon's: it stops branch
prediction from memorizing one file's shape, it gets closer to the cache-cold
behaviour that matters in practice, and it avoids anchoring on a single file
that may be unrepresentative. A fourth is ours: cross-module resolution is where
semantic analysis would be superlinear if it were superlinear anywhere.

**Three orders of magnitude in the default run, four on request.** 1k, 10k, 100k
lines by default; the scale tier adds 1M behind `--set=scale` (§4). A single
scale point cannot show a cache cliff, and Carbon's numbers fall 6.70 → 5.02 M
lines/s between 1k and 256k — the fall-off *is* the finding. The default run
stops at 100k for wall time rather than for principle, and §6.4's first finding
is what the flag turned up the first time somebody used it.

**Phase isolation at the compiler's own seams.** Each timer wraps the same
function the driver calls, and the isolation falls out of the signatures —
`Checker::resume(..).run()` takes `&Loaded` and returns a fresh `Checked`,
`monomorphize::run` takes `&Checked` and returns a fresh `Program` — so a
repetition cannot see the previous one's work and nothing has to be cloned. The
harness fills the parse cache (`parser::Cache`) before the semantic-analysis
timer starts, which keeps parsing out of that measurement, and builds the
standard library's snapshot once, as an analysis does (see "The prelude floor").

**Block the optimizer, but not the code under test.** Each result goes through
`std::hint::black_box`. Carbon has a sharper version — the barrier on the loop's
induction variable rather than the result, and the loop index made
data-dependent on the phase's return value — worth adopting if these rows ever
get tight enough for it to matter. At the current gap factors it would be noise
about noise.

**Report the fixed cost separately.** See below.

#### The prelude floor

Semantic analysis of *any* program pays for the standard-library modules it
pulls in, whether the program is one line or a hundred thousand. The suite
measures that cost on its own — a module with the corpus's three imports and a
trivial `main` — prints it in the header, and reports both a gross rate and a
rate net of it. At 1,000 lines the floor is most of the measurement; at 100,000
it is a rounding error, and the two figures converging is itself a check that
the floor came out right. It is what explains Carbon's otherwise puzzling result
that checking is *faster* at 16k lines than at 256.

**The standard library part of the floor is paid once per process.** Every
compilation opens with the same modules: the prelude and the built-in types'
modules, or for a snippet the whole library. `compiler::snapshot` loads and
checks them once, and each analysis resumes from that (`Loader::seeded`,
`Checker::resume`), checking only the modules after them. Syntax trees are
shared by `Arc`, so every thread reads the one snapshot. Ids come out the same
as a whole run's, so diagnostics and output don't move. `sema` measures the
resumed run, and the header prints the snapshot's one-time cost beside it.

What's left is the floor program's own imports past the prelude. `"node"`
names `platform/host`, which imports `core/fs`, `core/path` and `core/process`
for the effects its structs implement: about 3,800 lines on every compilation
that names a platform. They load after the program's own modules, so they
can't join the snapshot without changing the ids a program's types get.

| Revision | `sema` floor | `lower` floor | snapshot, once a process |
|---|---:|---:|---:|
| 2026-09-01 | 0.57 ms | | |
| `afb169cd`, 2026-10-03 | 5.52–5.84 ms | 6.0 ms | |
| snapshot, 2026-10-03 | 2.65–2.77 ms | 6.0 ms | 4.3–4.5 ms |
| shared tables (§6.15), 2026-10-03 | 2.01–2.08 ms | | 3.8–3.9 ms |
| JS emit (§6.17), 2026-10-03 | | 3.85–3.98 ms → 0.49–0.50 ms | |
| sema hot spots (§6.18), 2026-10-03 | 1.92–1.94 → 1.58–1.66 ms | | 3.54–3.66 → 3.17–3.28 ms |

The 2026-10-03 rows are two alternating `--quick --only=mixed` runs each, on
a shared machine at load 15–25. The growth since September is the standard
library: 11.2k to 45.6k lines, and the floor program now imports `NodeHost`.

### 3.2 What was deliberately not adopted

- **`criterion`, or any benchmark framework.** The dependency bar in the
  workspace manifest admits code generators and platform interfaces; a
  statistics library is neither. Warming up, repeating, and reporting a median
  with its spread is a hundred and fifty lines. The cost: no bootstrapped
  confidence intervals, no automatic outlier classification, no HTML report. A
  `--json` mode and the discipline of reading the dispersion column replace
  them.

- **Deterministic totals with randomized order.** Carbon's generator shuffles
  structure while holding the *total* count of every construct fixed, so two
  runs do identical total work. This suite uses a *fixed seed* instead, so two
  runs compile byte-identical source and the totals are trivially equal —
  stronger for run-to-run comparison, weaker for the one thing Carbon cared
  about: a fixed corpus can sit in a silent local minimum of the hash functions
  or the branch predictor. The escape hatch is the seed itself, reachable as
  `--seed=<hex>` or as a field in a saved corpus's manifest.

- **Re-randomizing per benchmark run so that ASLR noise shows up.** Same
  reason, same trade. This suite reports the spread of one binary's repetitions,
  not the spread across processes.

- **Hardware performance counters.** Carbon wires `libpfm` into google-benchmark
  and reports cycles and instructions, which is how a claim like "200–300 cycles
  per line" becomes checkable. Nothing here can do that without a dependency,
  and macOS has no `perf`. §7's sampling profiler runs stand in: they name the
  hot functions without quantifying them per byte.

- **~~Speed-of-light calibration benchmarks.~~ Adopted since.** They bound what
  the hardware can do at all; without them "232 MB/s" means nothing.
  `cli/benches/calibrate.rs` is five loops over the same corpus text the timed
  rows use — `memcpy`, byte-scan, token-write, node-write, alloc-pair — behind
  `--calibrate`. The interpretation rule went down before the numbers arrived,
  and the binary applies it itself, so nobody picks the reading after seeing the
  result.

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
| `cli/benches/corpus.rs` | Saved and pinned corpora: the manifest, the digest, discovery, `--record`, `--pin`, the size cap. |
| `cli/benches/calibrate.rs` | The speed-of-light ceilings: five bare loops over the same bytes, and the interpretation rule they are read by. |
| `cli/benches/compiler.rs` | The harness: warmup, repetition, median/MAD, the phase timers, the report. |
| `cli/benches/corpora/` | Eight checked-in corpora, 0.55 MB, capped at 2 MiB. |
| `cli/benches/pinned/` | Forty digest-pinned manifests — twenty parameter points at 100k and 1M — and no source. 15,546 bytes. |

`autobenches = false` in `cli/Cargo.toml` keeps the first three files *modules*
of the `compiler` target rather than bench targets of their own — Cargo infers a
target from every `.rs` file directly under `benches/`, and without it a plain
`cargo bench -p buri` would try to build three binaries with no `main`.

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
  --set=<name>      core | realistic | stress | native | saved | scale |
                    scale-full | full                       (default: core)
  --only=<text>     keep corpora whose label contains it
  --shape=<profile> one profile ad hoc, instead of a set
  --param <k>=<v>   override a dimension (repeatable; with --shape)
  --scale=<n>       target lines for --shape
  --seed=<hex>      seed for --shape
  --targets=<list>  js,macos-arm64,macos-x86_64,linux-x86_64,linux-arm64
  --record[=<name>] write the corpus into cli/benches/corpora/ and exit
  --pin[=<name>]    write a digest-pinned manifest into cli/benches/pinned/
  --rss             peak RSS and instructions per phase, untimed
  --calibrate       the speed-of-light ceilings, per corpus (§3.2)
  --alloc           allocations per line, per phase, untimed
```

`--alloc` needs the toolchain built with its counting global allocator, off by
default. The counter adds two atomic increments to every allocation in the
process, so a timed row taken with it on does not compare with one taken
without.

### The scale tier

```text
cargo bench -p buri --bench compiler -- --set=scale         # the sample
cargo bench -p buri --bench compiler -- --set=scale-full    # all forty
cargo bench -p buri --bench compiler -- --set=scale --rss   # and peak memory
```

Four orders of magnitude are what tell you whether a rate is a property of the
code or of the cache, and the fourth one costs minutes. So it is opt-in, and
**not** in `core` and not in `full`: a default run has to stay something a
contributor takes before a commit.

**The corpora are digest-pinned** (§3.1) — forty manifests in
`cli/benches/pinned/`, regenerated per run and checked against their recorded
SHA-256 before any timer starts. A mismatch stops the run.

**Forty is twenty points at two scales, and the points span the generator's
axes rather than repeating `mixed`.** A single profile measured at 1M answers
"does the rate hold as the program grows", not "which parameter is the rate a
function of" — and an axis that could be superlinear shows it only at the top.
So each of the twenty moves one axis:

| Points | The axis |
|---|---|
| `mixed` | The anchor. Every other point is a delta from it, and shares nothing but the generator. |
| `mixed-many-files`, `mixed-few-files` | Module count against module size: 15,437 modules at 1M against 186. |
| `mixed-libs`, `mixed-deep-graph`, `mixed-wide-graph` | Import-graph shape: clustered, deep, wide. |
| `struct-heavy`/`struct-light`, `enum-heavy`, `impl-heavy`, `match-heavy`, `string-heavy`, `list-heavy`, `long-bodies` | Construct-family weight — one kind turned up until it is most of the corpus. |
| `generic-blowup`/`generic-free` | Generics density: 235k monomorphized functions at 1M against 115k. |
| `derive-heavy` | Derive load, which only the native branch pays for. |
| `comment-heavy`/`comment-free`, `long-idents` | Surface: 46 bytes a line against 29, and bytes per token at a fixed token count. |

Sixteen of the twenty are named profiles from the table below. Four —
`string-heavy`, `list-heavy`, `long-bodies`, `generic-free` — are the `mixed`
profile with one weight moved, recorded that way in the manifest's `params`.
They are points rather than profiles because a profile earns a row in every
`--set=stress` run, and these had earned a scale row and no more. Each point's
two scales share a seed, so only the size differs between the two rows, which is
the whole comparison.

**A new scale point is a new manifest and nothing else.** The tier is every
`.txt` in that directory, filtered on the manifest's own fields, so a 10M row is
one `--pin=mixed-10M` away with no code change. It is deliberately absent: one
repetition of a 10M native row runs about three minutes, and the 100k/1M pair
already answers the question it would ask twenty times over.

**The protocol deviation is §2's, printed beside the rows it applies to.** Above
500,000 lines: at least 3 repetitions rather than 10, one warmup call rather
than two.

**Native rows are spent, not spread**, because a native row costs about thirty
times a JavaScript one.

The cross triples go to the anchor only: they settle whether a gap is codegen or
cross-compilation, and across forty corpora they would eat three quarters of the
wall time. Above 500,000 lines nothing takes them, the anchor included.

A native row at all goes to **seven of the twenty points**, and the other
thirteen manifests record `native = false`. The backend is a function of two
things this suite can move — the codegen unit count and the size of the IR
handed to it — so the seven span both: `mixed-many-files` (15,437 units at 1M)
and `mixed-few-files` (186) at the ends of the first, `generic-blowup` (235k
monomorphized functions) and `enum-heavy` (55k) at the ends of the second,
`derive-heavy` because `middle::derives` runs only on the native branch,
`struct-heavy` because layout and the ABI are native-only questions, and `mixed`
because it is the anchor. This is a *sampling* decision, and it reverses easily:
`--only=<point> --set=scale-full --targets=macos-arm64` takes the native row of
any of them by hand.

**The wall time belongs to the flag, not to the directory.** A sweep over forty
pinned corpora at a million lines takes twenty-five minutes, past the point
where anybody runs it before a commit. So:

| Command | What it covers | Wall time |
|---|---|---|
| `--set=scale` | the sample: the whole 100k tier, plus `mixed-1M` | ~9 min |
| `--set=scale-full` | all forty | ~25 min |
| `--only=<text>` | either of the above, cut to a point or a scale | seconds to minutes |

The sample is "every pinned corpus the standard protocol applies to, plus the
anchor above it", using the same 500,000-line threshold as the repetition
deviation, so the tier boundary is one number rather than two. `scale-full` buys
the other nineteen size comparisons, taken deliberately on a quiet machine when
a number is about to be written down.

### Peak memory

`--rss` reports the peak resident set size of each phase — §1's fourth column.
It is an **untimed pass**, taken before the timers and never beside them.

The figure comes from a subprocess: `--rss` re-runs this same binary once per
phase under `/usr/bin/time -l` and reads the maximum resident set size back.
Nothing asks in-process without a dependency — macOS has no `/proc`, `getrusage`
sits behind `libc`, and `ps -o rss` needs an entitlement. One phase per process
is the measurement rather than a workaround: a peak is monotonic, so the peak of
a process that stopped after `sema` *is* the cost of everything up to and
including `sema`, and the difference between two of them is what a phase added.
Sampling the current figure instead would miss whatever a phase allocates and
frees inside itself, which at these scales is most of the question.

On macOS the same child reports its instructions retired, and `--rss` prints
them over the `corpus` row. They're cumulative the same way, and they're the
column to compare two toolchains by (§8).

`--validate` is the one to run in a hurry. It proves the corpus is still valid
Buri after a language change, and it is fast because it compiles each program
once instead of ten times. A drifted generator shows up there as a list of
diagnostics rather than as a benchmark quietly measuring the error paths — and
so does a *saved* corpus that has stopped compiling, because `--validate` always
covers the checked-in half whatever `--set` you asked for. It also prints which
backends this binary has, which of the requested targets each can emit for, and
how much of the 2 MiB corpus budget is spent.

How much of the *pinned* half it covers follows `--set`: **0.4 s** under
`--quick` and no digests, **12.8 s** plain and the anchor's two, **27.3 s**
under `--set=scale`, and **4 min 20 s** under `--set=scale-full` for all forty.
Each is the fastest of three runs taken on 2026-09-01 at `0c66339d` on a machine
that was never fully idle, so all four are upper bounds. Against the figures
generator revision 7 recorded on 2026-08-31 — 0.4 s, 12 s, 26 s, 3 min 24 s —
the first three hold and **`--set=scale-full` is at least 27% longer**, the one
row somebody should re-take on a quiet machine before quoting it.
`--set=scale-full` also answers "is every pinned digest still good", so run it
after touching `generate.rs` — or after touching `formatting`, which since
`GENERATOR_REVISION` 7 is the same thing: `laid_out` is the last hand every
generated module passes through.

### The parameter space, and the profiles

A profile is a point in the generator's parameter space: `Params::default()`
with two or three fields moved. `Params` has about twenty dimensions — size and
distribution, a weight per construct kind, a size per construct, three surface
dials, and the reachability invariant — and `Params::default()` is
**byte-identical to the `mixed` corpus §6's numbers were taken over**, a promise
kept by regenerating and diffing. `--list` prints the profiles with their
parameters, and `--shape=<profile> --param k=v` runs a point that is not in the
table. An investigation worth watching becomes a profile.

A step sits between the two: a **parameter point** is a `--param` delta with a
name, a seed and a pinned digest, measured at 100k and 1M and nowhere else. Four
of the twenty points above are that, and they are not profiles because a profile
costs a row in every `--set=stress` and `--quick` run forever, while a point
costs four hundred bytes and a line in one table.

Two axes are deliberately *absent*. **Package structure**: the in-memory loader
uses `Loader::new(None, ..)` and never consults a workspace, so "many libraries"
means "many clusters in the import graph". **Parallelism**: the front end is
single-threaded, and §2 already covers it.

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
sema             semantics::resolve::Checker::resume+run   Loaded   -> Checked       (goal 2)
lower+js         monomorphize::run + actions::emit         Checked  -> JavaScript    (goal 3)
lower+<triple>   monomorphize::run + actions::prepare
                                   + Backend::emit         Checked  -> object bytes  (goal 3)
```

`actions::emit` is the same call `buri build --output=js` makes, `prepare` — and
therefore `middle::run` — included.

The native rows make the same two calls `actions::objects_of` makes below the
front end — `prepare`, the one place that picks the middle-end pipeline, and
`Backend::emit` — and stop there. **Nothing is linked and nothing is run**: the
link is the only host-only step, and goal 3 covers lowering rather than
producing an executable. The rows leave out the second `lower::run` that
`objects_of` performs for the cache keys, too: that is the build system paying
for content-addressing, not the compiler lowering the program.

Each repetition rebuilds the monomorphized program, because `prepare` mutates in
place and is not idempotent. So monomorphization sits inside every lowering row,
JavaScript and native alike, which is what keeps the three comparable.
`--split` subtracts it and reports
`mono | middle-A | middle-native | lower(IR) | emit`, to stderr, so
`--json --split` still emits one parseable document.

Three native triples by default — `aarch64-apple-darwin`,
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` — whatever machine
the suite runs on. A cross triple is *more* reproducible than the host one: the
host ISA comes from the running CPU's features, while a cross ISA is the
baseline for its triple. The refusal to cross-*link* stays in `link::can_link`
and `actions::native_ready`, which is what `buri build --output=native/linux-x86_64` on
a mac answers to.

**You can take a debug row only where the debug backend has a stencil library
for the triple** (`design/native/CODEGEN-STENCIL.md` §3.2). A triple with no
library gives a `skipped` row carrying the backend's own sentence. Of the five
triples you can ask the suite for, that is `macos-x86_64` alone: `linux-x86_64`
emits and gets timed like the other two natives (§6.1).

`Profile::Debug` selects the copy-and-patch backend and `Profile::Release`
selects LLVM, so a release row is an LLVM row, and only a toolchain built with
`backend-llvm` can take one. **There is no `#[cfg]` anywhere in the harness.** It
asks `backend::select` and prints `skipped: <diagnostic>` rather than testing a
feature, so the report says which rows *this* binary could not take instead of
changing shape with how it was compiled. Skipped rows go in their own `skipped`
array in `--json` rather than into `rows`.

The harness asks `Backend::missing_intrinsics` before any timer, for the same
reason it compiles the corpus before any timer: a backend that would have failed
must not be measured failing. `design/native/CODEGEN-STENCIL.md` §9 lists what
the current backend does not do.

One convention came out of a native row that used to skip. **`Too many return
values to fit in registers`** on either x86_64 triple, for `main`:
`Result<(), Str>` is three scalars, the SysV return convention has two, and
AArch64's eight had hidden it. Beyond `MAX_RET_LEAVES` a result now travels
through an out-pointer the caller passes and the callee writes, the rule the
runtime's C entries already followed. The threshold is a constant rather than a
question put to the ISA, so every test on every host walks the memory path.

---

## 5. How large the compiler is

The size census, so "the compiler got smaller" is a claim somebody can check.
Rust under `cli/src`, counted by a script rather than by a line counter this
repository would have to depend on. A line counts as a comment when it begins
with `//`.

**Re-taken whole on 2026-09-01, at `0c66339d`.** The `before` column is the
previous census, which still counted `compiler/backend/cranelift` — five files
and 8,634 lines the tree lost on 2026-08-29 — and nobody subtracted anything
from it by hand.

| Area | Files | Code | Comment | Blank | Total | Comment share | before, total |
|---|---:|---:|---:|---:|---:|---:|---:|
| `parsing` (lexer, parser, tree) | 5 | 5,343 | 1,333 | 450 | 7,126 | 19% | 6,572 |
| `compiler/semantics` | 11 | 10,387 | 2,825 | 764 | 13,976 | 20% | 11,413 |
| `compiler/middle` | 13 | 13,504 | 5,135 | 1,117 | 19,756 | 26% | 17,562 |
| `compiler/backend/js` | 4 | 6,092 | 1,645 | 371 | 8,108 | 20% | 7,214 |
| `compiler/backend/llvm` | 6 | 9,273 | 3,976 | 524 | 13,773 | 29% | 11,325 |
| `compiler/backend/stencil` | 18 | 14,411 | 5,715 | 935 | 21,061 | 27% | 18,033 |
| `build` | 14 | 8,540 | 2,955 | 724 | 12,219 | 24% | 11,062 |
| `commands` | 16 | 6,190 | 2,513 | 445 | 9,148 | 27% | 5,860 |
| `documentation` | 12 | 6,177 | 1,452 | 530 | 8,159 | 18% | 6,630 |
| `language_server` | 24 | 9,036 | 3,778 | 750 | 13,564 | 28% | 1,430 |
| shared, driver, stdlib glue | 16 | 6,953 | 4,037 | 689 | 11,679 | 35% | 7,700 |
| **`cli/src` total** | **139** | **95,906** | **35,364** | **7,299** | **138,569** | **26%** | **113,435** |
| `cli/runtime` | 19 | 13,756 | 11,504 | 1,460 | 26,720 | 43% | 6,954 |
| `cli/tests` | 43 | 21,399 | 9,884 | 2,020 | 33,303 | 30% | 21,492 |
| `cli/benches` | 4 | 3,654 | 1,229 | 271 | 5,154 | 24% | 5,038 |

**Two rows carry most of the growth, and neither is a compiler phase.**
`language_server` is **9.5×** what it was — 1,430 lines to 13,564 — and
`cli/runtime` is **3.8×**, 6,954 to 26,720: the reactor, the TLS client,
HTTP/1.1 and h2, WebSockets, the thread stacks and the scoped arenas §6.6 and
§6.7 measure. Against those the front end moved little: `parsing` +8.4%,
`semantics` +22%, `middle` +12.5%, `backend/js` +12.4%.

And the Buri-language side, which the compiler has to get through:

| | Files | Code | Comment | Blank | Total | before, total |
|---|---:|---:|---:|---:|---:|---:|
| standard library (`core/*`, `ui/*`) | 44 | 5,526 | 5,646 | 1,202 | 12,374 | 6,552 |
| test corpus (`cli/tests/**/*.buri`) | 5,315 | 63,602 | 15,935 | 10,745 | 90,282 | 25,654 |
| shipped documentation (`crates/docs/src/docs/**/*.md`) | 299 | — | — | — | 19,226 | 12,000 |

The test corpus is the row to read twice: **5,315 files against 1,021**, and
2,000 of the new ones are `cli/tests/formatting/generated`, a fixture directory
rather than hand-written Buri.

The three phases the goals name are **49,000 lines of Rust** between them —
`parsing` at 7.1k, `semantics` at 14.0k, and `middle` plus `backend/js` at
27.9k — against 35,500 at the previous census. That is the surface any
optimization wave has to work on, and it grew by 38% while every rate on this
page stayed inside its own dispersion or improved (§6.1). The **two** remaining
native backends are another 34.8k on top of it, down from three and 38k.

**The compiler also has to be built and shipped, and the 2026-08-29 removal
moved both.** Measured on the machine and toolchain of §6, from a clean `cargo
build --release -p buri` with default features, at the commit before the removal
and the commit after it:

| | before | after |
|---|---:|---:|
| dependencies (`cargo tree -p buri --edges normal`) | 38 | **0** |
| clean release build, median of three interleaved runs | 142.68 s | **73.94 s** |
| `buri`, as linked | 17.57 MB | 22.63 MB |
| `__TEXT.__text` — the machine code in it | 8.37 MB | **3.03 MB** |

The first two rows are the whole case for the removal. **The default toolchain
now resolves nothing at all**: the dependency bar in the workspace manifest is
back to zero admitted crates. And a clean release build is 68.7 s faster, a
little over half what it was. The 38 that went were Cranelift and its transitive
closure. Building the bench binary halved with them, 2 m 02 s to 1 m 01 s.

**Quote the last two rows together or they mislead.** Machine code fell by
5.35 MB, to 0.36× what it was, and dependency-derived constant and linkedit data
by 1.1 MB more — and the shipped binary still grew by 5.06 MB. One cause
explains both: the three baked stencil libraries are 11.93 MB of
`include_bytes!` data, byte-identical in the two builds, and before the removal
nothing in the `buri` *binary* selected the copy-and-patch backend, so the
linker dead-stripped them. Somebody checked that rather than assuming it — three
48-byte probes from `stencils-macos-arm64.bin` turn up in the newer image and
are missing from the older one. Quoting the total alone reads as a regression
the removal did not cause; quoting `__text` alone claims a saving the disk never
sees.

**Both sides of that were re-measured on 2026-09-01**, `cargo build --release
-p buri` with default features at `f9fffe1c` and at `0c66339d`, on this machine,
the two binaries `size -m`'d rather than reasoned about:

| | 2026-08-29, `f9fffe1c` | 2026-09-01, `0c66339d` | Δ |
|---|---:|---:|---:|
| dependencies (`cargo tree -p buri --edges normal`) | 0 | **0** | — |
| `buri`, as linked | 22,721,584 B | 26,798,320 B | **+4,076,736** |
| `__TEXT.__text` — the machine code in it | 3,104,388 B | 3,437,424 B | +333,036 |
| `libburi_rt.a`, `include_bytes!`d into it | 5,916,864 B | 9,097,192 B | **+3,180,328** |
| the three stencil libraries, likewise | 11,934,432 B | 11,934,781 B | +349 |

**Seventy-eight per cent of the toolchain's growth is one file.** The runtime
archive gained 3.18 MB because the servers program linked a reactor, a TLS
client, HTTP/1.1, h2 and RFC 6455 framing into it. Machine code is 8% of the
growth and the stencil libraries are 349 bytes of it. **The default toolchain
resolves nothing** at both commits, which is the row a reader should check first
when a binary grows by four megabytes.

---

## 6. Where the toolchain stands

Measured on an M-series MacBook (macOS, aarch64, 10 cores), release build, seed
`0x0b001a575eed0001`, protocol as §2. A gap of 1.0 means the row meets its goal;
below 1.0 means it beats it.

> **The native figures here were re-taken on 2026-08-29, after Cranelift was
> removed.** Every native row below is the copy-and-patch backend's
> (`design/native/CODEGEN-STENCIL.md`), measured against the same rows taken at
> the commit immediately before the removal, on this machine, over the same
> corpora, both binaries built `--release`. The front-end and JavaScript rows
> moved by less than their dispersion. Anything not re-taken says so where it
> stands and keeps its own date.
>
> **One command takes this table again.** A default run used to overflow the
> main thread's stack on `wide-match/10k`, so the rows had to be assembled out
> of `--only=` selections. **That is closed**, and somebody checked rather than
> assuming it — both binaries rebuilt and run on 2026-09-01 on this machine,
> each from its own tree, each from the bare command:
>
> | | `--validate` | a default run |
> |---|---|---|
> | `f9fffe1c`, 2026-08-29 | **aborts** — `fatal runtime error: stack overflow`, after `deep-nesting/10k` | **aborts**, the same way, 114 s in |
> | `0c66339d`, 2026-09-01 | **exit 0**, the whole corpus compiles, 12.8 s | **exit 0**, 150 s and 153 s on two runs |
>
> Every `mixed/100k` row below is the better reading of three processes whose
> every MAD is ≤ 2.8%, inside §2's ±5%.

> **Generator revision 8, 2026-10-02 — a break in the series, bytes only.**
> Effects moved to `platform/effect` and `main` takes its platform's host, so
> every module's effect import is `platform/effect`, which the formatter sorts
> after the `core/*` imports, and `main.buri` imports `NodeHost` from `"node"`.
> **All eight** saved corpora were re-recorded and **all forty** pinned
> manifests re-pinned. `lines` did not move on the anchor (`mixed-100k` 101,074,
> `mixed-1M` 1,010,518), and `bytes` grew by 1.3% (`mixed-100k` 3,516,842 →
> 3,562,098), so every reading below stands and nobody re-took the table.

> **Generator revision 7, 2026-08-31 — a break in the series, announced, and
> the first one that moves a corpus's *shape* rather than only its bytes.**
> Every module now leaves `generate.rs` through `formatting::source`, so a
> generated corpus is what `buri format` writes: four spaces, a sorted import
> run, a `derive` above the declaration it is about, and a body broken where the
> printer breaks it. §3.1's rule applies, and this revision followed it: **all
> eight** saved corpora re-recorded, **all forty** pinned manifests re-pinned.
>
> **The line count barely moved, and that is the number the goals are stated
> in.** `lines` moved **+0.32%** on the anchor (`mixed-100k` 100,755 → 101,074;
> `mixed-1M` 1,007,259 → 1,010,518), less than 0.5% on thirty-eight of the forty
> pinned corpora, and at most **+1.54%** on any of them
> (`mixed-many-files-100k`). So every lines/s reading below is comparable with
> one taken at revision 6 to inside half a percent — well inside the dispersion
> the protocol reports — and nobody re-took the table.
>
> **The byte count moved where the layout is what the point is about**, and
> `modules` fell on nineteen of the twenty points — 360 → 348 on the anchor,
> 361 → 287 on `struct-heavy` — because a module reaches its line target with
> fewer declarations once its bodies are laid out. `struct-heavy` −12.1% of
> bytes and `long-idents` −10.7% are the two large ones; `match-heavy` +5.2% is
> the other direction, a match arm gaining four columns of indent rather than
> two. Everything else is inside ±2%.
>
> **One saved corpus changed what it stresses**, deliberately.
> `many-small-fns-1k` is 30,492 → 17,205 bytes and 3 modules → 2, because `buri
> format` writes a one-expression function over three lines: the per-function
> line estimate went 2 → 4, so a 1,000-line budget now buys 250 tiny functions
> where it bought 500. Four lines per tiny function is the density a *formatted*
> repository of them has, and the old estimate would have made `--scale=n` mean
> 2n lines.
>
> **What it buys** is a line rate quoted over source somebody would check in.
> `cli/benches/corpora` now sits inside
> `cli/tests/language/corpus.rs::every_source_in_the_repository_is_formatted`,
> and passes. It stays outside `BURI_BLESS`, because the fix for a drift in
> generated output is `--record`, not laying the file out where it sits.
>
> **What it costs** is about a fifth on top of every validation — `--validate`
> 10 s → 12 s, `--set=scale` 21 s → 26 s, `--set=scale-full` 2 min 47 s →
> 3 min 24 s — all of it at work-list construction, outside every timer. The
> other cost is a coupling: **a change to `formatting` is now a change to the
> generator**, and takes this same ceremony.

> **Generator revision 6, 2026-08-31 — a break in the series, announced.**
> An import that names a surface names the module again, so `core/list/lib.buri`
> is `core/list`. Every generated module imports four or five standard library
> modules across a boundary, so every import line is nine bytes shorter; nothing
> inside `//bench` moved, which is why three corpora came back byte-identical
> where revision 5 moved all eight. All eight saved corpora were re-recorded and
> all forty pinned manifests re-pinned. `lines` and `modules` are identical for
> every one of them — checked over the diff, not assumed — so every reading
> below is still comparable with one taken at revision 5.

> **Generator revisions 2 to 5 each broke the series, and each moved bytes
> only.** `core/cap` became `platform/effect` (rev 2, 2026-08-23); `self` stopped
> writing its type (rev 3, 2026-08-27); an enum variant stopped carrying
> `export` (rev 4, 2026-08-27); every import named a file, `core/list` →
> `core/list/lib.buri` (rev 5, 2026-08-30). §3.1's rule applies to each: the
> saved corpora the change touched were re-recorded and the pinned manifests
> re-pinned, while a corpus whose bytes never moved stayed where it was, because
> byte-stability across a generator change is the whole point of saving one.
> Revision 5 is the edge case — `few-large-fns-1k` and `wide-match-1k` carry no
> import and not one byte of their source moved, but a corpus digest folds each
> module's *path* as well as its text, so both were re-pinned anyway. For all
> four revisions `lines` and `modules` are identical over all forty pinned and
> all eight saved corpora, so a rate quoted in lines/s is unmoved.

### 6.1 Where every goal stands

mixed/100k, the authoritative corpus, on the machine and protocol above.
**Re-taken 2026-09-01 at `0c66339d`**, from the default run §6's note describes.
The `2026-08-29` column is what this table said at `f9fffe1c`, and the column
after it is that same commit's binary re-run today, which is what separates a
machine from a compiler.

| Phase | Goal | 2026-08-29, `f9fffe1c` | `f9fffe1c` re-run today | **2026-09-01, `0c66339d`** | Gap |
|---|---:|---:|---:|---:|---:|
| lex | (10 M shared) | 12.06 M | 12.58 M | **12.88 M** | **MET** |
| lex+parse | 10 M | 6.40 M | 6.04 M | **6.36 M** | 1.57× |
| sema | 1 M | 1.32 M | 1.15 M | **1.35 M** | **MET** |
| lower+js | 100 k | 311 k | 284.1 k | **255.0 k** | **MET** |
| lower+macos-arm64 | 100 k | 133.3 k | 126.4 k | **135.2 k** | **MET** |

**The lex and lex+parse rows moved on 2026-10-03**: 1.86× and 1.56× the rate
of the commit before, measured side by side. §6.14 has the readings; this table
keeps its September figures because nobody re-took the other rows that day.

Two of the three goals are still met, and the third is met on **both** lowering
backends rather than on the JavaScript one alone. Lex+parse started at 1.45 M
lines/s and is 4.4× that now; native lowering started at nothing measurable,
because the realistic corpora could not be compiled natively at all.

**One row fell, and it is `lower+js`.** 311 k to 255 k is −18.0%, and two things
caused it: this machine on a different morning, and the compiler. The middle
column separates them — `f9fffe1c`'s own binary, rebuilt and re-run today, reads
**284.1 k** where this table recorded 311 k on the day, so roughly half the fall
is the machine. Pricing the toolchain's half meant running the two binaries
**A/B/A/B** in one sitting, `--only=mixed/100k`, four processes, 2026-09-01 —
the protocol §6.6 uses — taking each compiler's better median and discarding
every leg whose MAD exceeded §2's ±5%. Both columns below come from that
sitting, which is why its `0c66339d` figure is 248.9 k where the table above,
whose better reading came from the default run, says 255.0 k:

| Phase | `f9fffe1c` | `0c66339d` | Δ in rate |
|---|---:|---:|---:|
| lex | 12.58 M | 12.57 M | −0.1% |
| lex+parse | 6.04 M | 6.36 M | **+5.3%** |
| sema | 1.15 M | 1.35 M | **+17.4%** |
| lower+js | 284.1 k | 248.9 k | **−12.4%** |
| lower+macos-arm64 | 126.4 k | 135.0 k | +6.8% |
| lower+linux-x86_64 | 123.7 k | 128.0 k | +3.5% |
| lower+linux-arm64 | 128.3 k | 130.7 k | +1.9% |

**The JavaScript emitter is 12.4% slower per line and every other row is flat
or better**, which is what makes the one that fell believable rather than a bad
afternoon. **No budget on this page is stated over `lower+js`** — goal 3 is, and
the row meets it by 2.5×. It is not a volume effect: between the two commits the
anchor's monomorphized function count *fell* 13,162 → 12,735 and its emitted
JavaScript grew only 1,317,286 → 1,347,614 bytes, so the same row is **+18.3%
per monomorphized function** and **−10.7% per emitted byte**. The `ctx`
parameter every module function now threads, `println`'s `Result`, the actor
runtime in `runtime.js`, and the `async`/`await` printed around every call that
can wait are what arrived in `backend/js` over those 413 commits.
Which of them owns the 12.4% is a profile away (§7).

**`lower+macos-arm64` moved because a new emitter replaced the old one.**
Cranelift read 62.2 k lines/s on this machine on the day of the comparison and
the copy-and-patch backend reads **133.3 k**, which is 0.47× the time and 2.14×
the rate — within noise of `design/native/CODEGEN-STENCIL.md` §1's "about 0.43×
Cranelift's", and the first time goal 3 has been met natively here. The series
breaks at the change of emitter, and this page marks the break rather than
reading through it.

| Corpus | Target | Cranelift | copy-and-patch | ratio | after, lines/s |
|---|---|---:|---:|---:|---:|
| mixed/1k | macos-arm64 | 17.03 ms | 6.50 ms | 0.38× | 159.6 k |
| mixed/10k | macos-arm64 | 159.58 ms | 57.20 ms | 0.36× | 176.9 k |
| **mixed/100k** | **macos-arm64** | **1,620.26 ms** | **756.12 ms** | **0.47×** | **133.3 k** |
| mixed/100k | linux-arm64 | 1,584.50 ms | 799.69 ms | 0.50× | 126.0 k |
| mixed/100k | linux-x86_64 | 1,714.87 ms | 794.89 ms | 0.46× | 126.8 k |
| many-small-fns/10k | macos-arm64 | 94.57 ms | 48.90 ms | 0.52× | 206.2 k |
| enum-heavy/10k | macos-arm64 | 312.03 ms | 152.83 ms | 0.49× | 66.5 k |

Dispersion is MAD ≤ 1.6% on every row, and the headline row was taken twice in
independent processes — 756.12 ms and 758.12 ms. **All three emitting triples
clear goal 3**; `macos-x86_64` is absent because it has no stencil library and
says so in its own words (§4). `enum-heavy` is the one row still under the goal,
and it halved like the rest.

**The tuned corpus is not what produced this.** A pinned seed can sit in a local
minimum, so a copy-and-patch result gets re-taken on freshly seeded 100k
repositories before anybody writes it down. Five seeds minted for the comparison
and never used before, three shapes between them:

| Shape | Seed | ratio | after, lines/s |
|---|---|---:|---:|
| mixed | `0x29a8206b17c0f001` | 0.53× | 116.0 k |
| mixed | `0x29a8206b17c0f002` | 0.49× | 123.9 k |
| mixed | `0x29a8206b17c0f003` | 0.46× | 133.8 k |
| struct-heavy | `0x29a8206b17c0f004` | 0.50× | 180.9 k |
| derive-heavy | `0x29a8206b17c0f005` | 0.45× | 125.2 k |

The median of the five is **0.49×** against the tuned corpus's 0.467×, and
every one of them clears goal 3 at 116.0–180.9 k lines/s. `derive-heavy` — the
profile `middle::derives` alone pays for — is the *best* of the five rather than
the worst, the opposite of what overfitting to `mixed` would look like.

### 6.2 The dev and release configuration, both halves measured

- **Dev: the copy-and-patch backend, whole-binary link, per-unit emit.** It has
  no optimization dial to set — no instruction selection, no register allocator,
  no mid-end to skip (`design/native/CODEGEN-STENCIL.md` §1). **Three readings
  in this bullet are Cranelift's**, from before the flip, and nobody re-took
  them because the emitter under all three has changed: LLVM at `-O0` was
  2.1–4.9× slower to lower on the shapes that decide it; `opt_level = "speed"`
  cost 16–95% of native lowering and *lost* 2.6% of runtime (§6.4); and the path
  ran the four kernels at 0.91× of bun. What did get re-taken is the pair the
  change was made for: emission at **0.47×** Cranelift's time (§6.1), and the
  run side at **1.26×** Cranelift's over the four comparable kernels below.
  Measurement refuted both erasing generics in the dev profile (2–10× measured
  runtime cost, and a second value model through both backends) and moving
  instantiation placement (worth one unit of blast radius, and weak symbols
  would cost the direct branches).
- **Release: LLVM at `-O2`.** **10.55×** a cold dev build and **1.07× a no-op
  one**, for 1.84× the runtime over the four kernels below and 0.35× the
  artifact size — that last from before the flip and not re-taken. It lowers at
  5.5 k lines/s on the headline corpus, inside the 3.9–6.8 k band this row has
  always quoted, which is 18× under goal 3 and is the price of LLVM's optimizer
  rather than of this repository's lowering. The path a developer iterates on
  meets the goal. Dev emission is 22.3× release emission now, against 11.2×
  before the removal.

**The end-to-end numbers behind those two ratios.** No 100,000-line repository
ships in the tree, so this one was assembled out of the checked-in `mixed-10k`
corpus: ten copies, one package each, module paths rewritten — **101,190
non-blank lines, 10 packages, 370 modules**, every package a `MACOS`/`ARM64`
binary. Cold is `buri clean` plus `rm -rf .buri out`; no-op is the same command
again with nothing changed.

| Configuration | cold, median | no-op, median |
|---|---:|---:|
| dev, Cranelift (before) | 2.964 s | 0.388 s |
| dev, copy-and-patch (after) | **2.107 s** | 0.398 s |
| `--release`, LLVM `-O2` | 22.228 s | 0.426 s |

The cold build improves by 29% where emission improved by 53%, and the gap is
the finding rather than a discrepancy: at ten packages on ten cores the build is
parallel, and the front end, the action cache and the link did not move. So the
release-over-dev cold ratio **rises to 10.55×** from 7.9×, and the no-op ratio
is 1.07× against 1.15×. A single 10k package, for scale: 0.272 s cold and
0.087 s no-op.

**The run side, on six kernels written for the comparison.** The four programs
behind the 1.38× that `design/native/CODEGEN-STENCIL.md` §13 records **have no
harness in this repository**, so the six below are a fresh series and their
geomean is not that number re-taken. Same six sources through three code
generators, one machine, one afternoon: whole-process wall clock, median of
five, macOS arm64. The LLVM column is `-O2`, because `--release` is the only
optimized native path here — a harder bar than the literature's, not the same
one.

| Kernel | Cranelift dev | copy-and-patch dev | LLVM `-O2` | ÷ Cranelift | ÷ LLVM |
|---|---:|---:|---:|---:|---:|
| primes, trial division to 2,000,000 | 175.1 ms | 188.8 ms | 184.7 ms | 1.08× | 1.02× |
| n-queens, n = 12 | 339.5 ms | 383.1 ms | 260.1 ms | 1.13× | 1.47× |
| matmul, 260 × 260 through `list.get` | 151.1 ms | 173.5 ms | 125.3 ms | 1.15× | 1.38× |
| `map`∘`filter`∘`map`∘`fold`, 500 × 20,000 | 51.0 ms | 92.0 ms | 16.7 ms | **1.80×** | **5.51×** |
| **geomean, those four** | | | | **1.26×** | **1.84×** |
| `str.concat` × 3,000,000 onto a unique `Str` | 26.6 ms | 29.5 ms | 13.2 ms | **1.11×** | 2.23× |
| `str.concat` + `str.fromInt` × 1,000,000 | 59.6 ms | 53.5 ms | 47.1 ms | **0.90×** | 1.14× |

The shape `design/native/CODEGEN-STENCIL.md` §13 describes holds, and has
tightened. Three of the four are within 1.08–1.15× of Cranelift, and **the whole
of the remaining gap is the `core/list` closure pipeline** — the surface
`rtcall.rs` deliberately does not inline. Cranelift itself is 1.46× LLVM `-O2`
on the same four against the copy-and-patch backend's 1.84×, so most of the
distance to release is the optimizer rather than the emitter. The two concat
rows are the in-place append port measured: both are at or better than
Cranelift's, and the row whose left operand stays unique comes out *ahead*. Per
append the cost went from 0.659 µs to 0.0098 µs — 67× less, and growing with
bytes rather than quadratically with appends.

**`buri test` defaults to the native dev backend**, since 2026-08-21. A suite
that names no platform compiles with the dev backend and runs as a binary, and
since 2026-09-03 there is no fallback: a toolchain that cannot build for its own
host, or a program the backend has no body for, gets a refusal naming what is
missing rather than a JavaScript run with a note (`commands/test.rs`;
`design/native/ARCHITECTURE.md` §4). The number that paid for the change is the
incremental one: a one-line edit at 104k lines is 502 ms to verdict native
against bun's 622 on the fast suite and 1,484 against 1,742 on the compute
suite, the first measurement here where the native compile column is the faster
one.

### 6.3 What remains open, in the order it matters

- **The `core/list` closure pipeline**, which is now the whole of the dev
  backend's remaining run-side gap: 1.80× Cranelift and 5.51× LLVM `-O2` on the
  fused-pipeline kernel while the other three sit at 1.08–1.15× (§6.2). It is
  `rtcall.rs`'s stated exclusion rather than a defect, and it is the one shape
  where the exclusion costs a reader something visible.
- **`lower+js`'s 12.4%.** The one row on this page that fell over the
  concurrency-and-servers program (§6.1). It needs a profile first (§7) over the
  JavaScript lowering call, to say whether the threaded `ctx` parameter,
  `println`'s `Result`, the actor runtime, or the `async`/`await` around calls
  that can wait own it.
- **The producer half of fusion.** `range` is still materialized.
- **Derived `Show`**, which needs the design decision in §6.4 rather than more
  tuning.
- **~~`buri_rt_list_get`.~~ Closed 2026-08-30, on both backends.** It was the
  whole of the matmul kernel's remaining gap, an out-of-line call that
  bounds-checks and `memmove`s one element into an `Option` payload, twice per
  inner iteration. Both backends now open-code it out of the same bounds test
  and load `list.map`'s loop already uses. The dev backend went first
  (`4aa877f`, `3ba487e`): `matmul` at **0.468×** its old time, `queens` at
  **0.628×**, and four held-out kernels written after the fix at a geomean of
  **0.519×** — a *larger* win than the tuned pair's 0.542×. The release backend
  followed (`3b262681`): **0.255×** over the six kernels that index a list, the
  held-out four at **0.250×**. A counted element still takes the call on both
  backends, because the runtime entry retains through the glue it is handed and
  the open-coded sequence does not, so that half is a reference-counting
  question rather than a codegen one. §6.2's three-generator table predates all
  three commits.
- **~~Realistic native lowering's last 1.72×.~~ Closed 2026-08-29.** 88% of the
  row sat inside Cranelift's own `define_function` and 42% inside regalloc2
  alone, so the lowering this repository owned was not the cost. **The emitter
  changed instead**: a copy-and-patch one with no register allocator in it at
  all, the row reads 133.3 k lines/s, and goal 3 is met natively for the first
  time (§6.1). It stays here because the reason it closed is the finding — the
  gap was in a dependency's design, and no amount of tuning on this side of the
  seam was going to reach it.
- **Lex+parse's last 1.34×.** §6.14 broke the old ~6 M plateau without a
  design change and names what is left: the lexer is half the phase, and the
  next allocation worth removing is a declaration's `docs: Vec<String>`,
  which reaches every consumer of the syntax tree.

### 6.4 Three findings that transfer

This page does not keep the rounds that produced the numbers above as a
chronology. An earlier revision numbered those rounds §6.1 through §6.9, so a
citation to one of them lands here. Three findings are worth more than the
numbers they produced, because each is a shape rather than a measurement:

**Per-unit work over a whole-program array is Θ(units × functions), and it hides
until it does not.** Two scans in the then-current native backend walked all of
`program.funcs` *per codegen unit* — one to collect the unit's own functions and
allocate a `vec![None; program.funcs.len()]` linkage table beside it, one to
build the text whose hash is the unit's cache key. At 100k lines that is
360 × 13,162 ≈ 4.7 M steps and nobody notices; at 1M it is
3,590 × 132,396 ≈ 475 M — a hundred times the work for ten times the program,
over an array too large to stay in any cache, so the constant is large as well
as the growth. The fix is `ir::Program::funcs_by_unit`, which buckets every
function index by its owning unit in one pass; both scans then read a row of
about thirty-seven entries. Native lowering at 1M went from 30.2 k to
**52.2 k lines/s**, which is the 100k rate to within noise, and the cache key
did not move — a row is ascending in function index, which is the order the
discarded filter yielded, so the bytes hashed are the same bytes. The general
statement: *the rate was a function of the unit count rather than of the program
size*, and that is the signature of this shape.

**Derived `Show` costs a constant per rendered field, not per variant, and the
cost is in the backend rather than in the expansion.** On a wide-enum corpus,
derived `Show` costs 4.7× the entire native lowering row while `Equal`, `Ordered`,
`Hash`, `ToJson` and `FromJson` together cost nothing measurable. Three
measurements say what it is not. Generating the expansion costs 29 ms and
lowering it to IR 32 ms more, while *emitting* it costs 1,221 ms more — forty
times the expansion that caused it. Regrouping 512 total variants from
128 enums × 4 to 8 × 64 costs the same to within 3%, so it is not superlinear in
function size and a per-variant helper split will not work. And payload-free
derived `Show` is free (231.0 ms against 234.9 with no derive at all) while one
field costs 33 ms, two 72 and four 163 — about **0.07 ms per hole**. So the
lever is CLIF volume and live ranges per hole, and the two routes left are a
variadic join that takes its parts through memory or a descriptor-driven
renderer emitted once, which is what the JavaScript backend already does. That
is a decision about `middle::derives`'s premise rather than an optimization.

**Measurement refuted `opt_level = "speed"` on the dev backend, on both halves
of the trade.** It cost 16–95% of native lowering. What it returned, over the
four kernels: primes −3.6%, n-queens −4.3%, matmul ±0%, and the fused pipeline
**+34%** — the one shape the fusion pass had just made fast, regressed by a
third because the egraph mid-end rewrote the fused loop into something its
register allocator liked less. The total was **+2.6%**: the suite slower, not
merely not-faster. The dial belonged to a backend that is gone, and the finding
outlives it because it is the shape rather than the number — **an optimizer in
the debug quadrant has to pay for itself on the run side, and this one did
not**.

### 6.5 Measured dead ends, recorded so they stay dead

- **Branchless reference counting**: +15%.
- **`opt_level = "speed"`** on the dev backend: +20% lowering, and §6.4's
  runtime regression. This and the row below were dials on the Cranelift backend,
  removed 2026-08-29. They stay here because a deleted dead end gets
  rediscovered.
- **`regalloc_algorithm = "single_pass"`**: silently inert since Cranelift
  0.123, which withdrew the value — the line was set and had no effect. It went
  with the backend and with the document that recorded it
  (`design/native/CODEGEN-STENCIL.md` §13).
- **Erasing generics in the dev profile**, and **moving instantiation
  placement**: both refuted in §6.2.
- **Scanning comments eight bytes at a time** for their line break, and
  **skipping the parser's end-of-stream clamp** on `peek`: both 0.0% on
  lex+parse (§6.14).

### 6.6 What the multi-threaded fork costs, 2026-08-30

Reference counting became **two counts behind one branch** on bit 63 of the
block's `cap` (MEMORY.md §5.1, "The shared fork"). At the time of this
measurement nothing set the bit, so what follows is the price of the *branch*
alone. **§6.7 is what changed** — a program that can reach a task boundary now
marks every block it allocates — and it leaves every number below standing,
because a program with no `core/tasks` in it still takes the unshared arm and
every corpus measured here is one.

**The stated budget was 3% on every row of `--set=native`, and it is 3% on four
of them.** The fifth, `lower+macos-arm64-release`, carries a budget of its own —
**amended 2026-08-30** to the range measured below, +16.6% … +26.3%, argued in
§6.6.1.

Protocol: `--only=mixed --set=native --targets=macos-arm64 --json`, the same
machine and §2's rules, run **A/B/A/B** — the baseline toolchain, this one, the
baseline again, this one again — so that drift shows up as a disagreement
between a compiler's two readings rather than as the difference between the two
compilers. Each cell is the better of that compiler's two run medians. Six
corpora at 10k.

| Phase | range over the six | median | budget |
|---|---:|---:|---|
| `lex` | −3.8% … +0.1% | −0.4% | **met** |
| `lex+parse` | −2.8% … +1.3% | −0.3% | **met** |
| `sema` | −4.9% … +8.2% | +2.2% | **met** |
| `lower+macos-arm64` (dev) | −5.1% … +7.6% | −0.4% | **met** |
| `lower+macos-arm64-release` (LLVM) | **+16.6% … +26.3%** | **+21.3%** | **met**, against an amended budget |

The first four rows have no direction — the front-end rows moving by ±4% in
both directions is what this machine's drift looks like over a twenty-five-minute
run. They are the control, and they are why the fifth row is credible.

**Goal 3 is unmoved.** §6.1's goal-bearing lowering row is `lower+macos-arm64`,
the development backend, and it is one of the four. The row that moved is the
*release* build's lowering, which no goal on this page is stated against.

**Why the release row and not the dev one.** The stencil backend copies a
stencil per IR operation, and the fork made three stencils longer rather than
making more of them: the emitter's work is unchanged and the row says so. The
LLVM backend hands `opt` its IR, and the fork adds two basic blocks and about
six instructions to *every reference operation in the program* — which roughly
doubles the IR of the single most common operation the emitter produces. The
`default<O2>` pipeline's cost is superlinear in block count, and 21% is what
that comes to.

**Making the shared arm a call bought half of it back.** The atomic sequence can
be open-coded in the IR beside the fork, or the fork can call
`buri_rt_incref`/`buri_rt_decref`, which fork again on the same bit. Both went
against the same baseline under the same protocol:

| Shared arm | `lower+macos-arm64-release` | median |
|---|---:|---:|
| open-coded saturating `atomicrmw` | +28.8% … +85.7% | +46.3% |
| **cold call into the runtime** (landed) | +16.6% … +26.3% | **+21.3%** |

The machine code on the arm every program actually takes is identical either
way, so the call is a pure win and is what shipped. It also leaves one atomic
sequence per backend rather than two — MEMORY.md §5.1's "open-code the fast
path, call the cold one" applied to a path that is cold by construction.

**What it costs the emitted program: two instructions per reference
operation**, on both instruction sets and both backends. Read off the objects
rather than inferred:

```text
aarch64 (stencil `st_incref`, and LLVM's inlined `decref`)
    ldur  x9, [x8, #-0x8]        // cap, the word beside the count
    tbnz  x9, #0x3f, <shared>    // never taken

x86-64 (stencil `st_incref`)
    cmpq  $0x0, -0x8(%rax)
    js    <shared>
```

The load takes the word next to the count, in the same sixteen-byte header, so
it sits on a cache line the operation was going to touch. The branch is
perfectly predicted, because its answer never changes. And both emitters mark
the unshared arm hot, so it falls through and the shared arm goes after the
tail.

**And what the emitted program does with it: it gets faster.** Both toolchains
built two Buri programs and ran each five times, alternating — `allocs`, whose
loop is three inlined `decref`s and an allocation per iteration, and `rcloop`,
whose counts `middle::rc` elides entirely, as the control. Best of five,
seconds:

| program | backend | before | after | Δ |
|---|---|---:|---:|---:|
| `allocs` (50 M iterations) | dev / stencil | 1.795 | 1.081 | **−39.8%** |
| `allocs` (50 M iterations) | LLVM release | 1.711 | 0.605 | **−64.6%** |
| `rcloop` (control, no counts) | dev / stencil | 0.423 | 0.431 | +1.9% |

Read that as one **combined** figure: the fork's two instructions against the
per-thread caches of MEMORY.md §5.4, which turn an allocation-and-free pair into
a list pop and a list push. The caches pay for the fork many times over on any
program that allocates, and the control — which has neither a count nor an
allocation — does not move. What the table cannot separate is the fork's own
runtime cost, because this language has no allocation-free reference traffic to
isolate it in: `middle::rc` elides exactly that.

**Two earlier runs went in the bin.** The machine's load average reached 136
while other work shared it, and the same binary timed 2.5 s and 20.7 s inside
one alternating sequence, which §2's dispersion rule says to discard. The table
above comes from a later run whose five readings per cell agree to ±8%.

#### 6.6.1 The amendment, and what it is answerable to

The 3% rule stands, unchanged, on the other four rows and on every row a later
change to `--set=native` gets measured against. The amendment covers one row's
budget: `lower+macos-arm64-release` is held to **+16.6% … +26.3%**, median
+21.3% — **accepted by Nick, 2026-08-30**. Three things carried that acceptance:

- **The spending is compile time, and only in one backend.** The release row is
  `opt`'s cost on the IR the emitter hands it, and the shared-RC branch adds
  **two basic blocks per reference operation** to that IR — roughly doubling the
  most common operation the emitter produces — against a `default<O2>` pipeline
  whose cost is superlinear in block count. The emitter's own work is unchanged
  and the dev row, one stencil copy per operation, says so at −0.4%.
- **What the shipping program pays is two instructions**, on both instruction
  sets and both backends, read off the objects: a load of the word beside the
  count and a bit test, on a cache line the operation was going to touch, on a
  branch that is perfectly predicted, because within one program its answer
  never changes. That is the unshared path, and it is the path every program
  that does not use `core/tasks` takes (§6.7).
- **And it pays them into a profit.** Beside the per-thread caches of MEMORY.md
  §5.4, an allocation-heavy program's run time falls **39.8%** on the dev backend
  and **64.6%** on the release one, while the allocation-free control does not
  move. A one-off compile-time cost buys a per-execution runtime win, which is
  the trade this page exists to make visible.

The amendment is **not** a widening of the rule. A row with a budget of its own
has its next regression measured against *this range* rather than against 3%, so
a second change of this size here is a second decision and not a rounding error.
The alternative that lost was to hold 3% and leave the row permanently red:
nobody reads a budget nothing can meet.

### 6.7 What a scope costs, 2026-08-31

`core/alloc::scoped` serves the blocks its body allocates out of its own
mappings and gives them back in bulk (MEMORY.md §7.2.1). Two questions, and
this section answers both with the same program §6.6 used:

**Stated budget: no more than 5% on `allocs`, and a scope must not be a
pessimisation.** Both met, at **+3.7%** and **+8.7%** respectively.

| program | before | after | Δ |
|---|---:|---:|---:|
| `allocs` — 50 M allocate-and-free pairs, **no scope in the program** | 1.0018 s | 1.0393 s | **+3.7%** |
| the same 50 M allocations, **a scope per batch of 100** | 1.0444 s | 1.1357 s | **+8.7%** |

The first row is A/B/A/B against a toolchain built from `HEAD`, medians of five
alternating readings, dev/stencil, macOS arm64. The second is two binaries from
the **same** toolchain — `cmd/plain` and `cmd/scoped`, identical but for the
`alloc.scoped` around each batch — so it is not a before-and-after at all. It is
what a scope costs the program that opens one: 500,000 scopes, **183 ns each**,
covering `create`, `enter`, `leave`, `release` and two uncontended mutexes.

#### 6.7.1 Three shapes were measured and two were thrown away

Every number below is the first row of the table above, on the same machine in
the same sitting. Each rejection is a fact about this platform rather than a
preference.

| where the "which scope am I in" question lives | `allocs` |
|---|---:|
| a `thread_local!` of its own | **+12.0%** |
| folded into G2's per-thread block cache | +4.1% |
| the same, plus a process-wide "any scope ever" latch | **+3.7%** |

**A second thread-local costs 2.4 ns on an allocate-and-free pair**, about a
third of what the pair costs: on macOS a `thread_local!` access is a call to
`tlv_get_addr` and not a register-relative load, and the allocation path was
already making one for the block cache. Folding the arena into `Cache` makes it
one access and a branch on a word already in a register, which is the whole
difference between the first two rows.

The third row sits **within the noise of the second, and stays anyway.**
`scopes_exist()` is a relaxed load of a word written at most once in a process's
life, and it buys the claim rather than the 0.3%: a program that never calls
`scoped` takes the two lines it took before this slice, readable off
`buri_rt_alloc` rather than inferred from a profile.

#### 6.7.2 The pool is why the second row is 8.7% and not 116%

The first working version **mapped and unmapped a 64 KiB block per scope**, and
the second row measured **2.2554 s against 1.0444 s** — 2.2× the same program
without scopes, or **2.4 µs a scope**, which is two system calls and nothing
else.

`ARENA_POOL` is eight standard blocks — 512 KiB, stated and bounded — that a
released scope hands to the next one instead of to the kernel. It is the third
time this runtime makes that trade (G2's per-thread block caches, B7's
thread-stack pool), for the same reason each time: **the common path should
make no system call.** With it, a scope's 2.4 µs becomes 183 ns.

One correctness consequence is worth stating beside the number, because it is
the kind that would not have shown up in a benchmark: a mapping fresh from the
kernel is zero-filled and an arena's window only moves forward, so before the
pool existed `buri_rt_alloc_zeroed` inside a scope could be a bump and nothing
else. A **pooled** block holds the last scope's bytes, so it zeroes.
`a_zeroed_block_in_a_scope_is_zero_even_out_of_the_pool` is that, as a test.

### 6.8 Where the program ends, 2026-09-01

The concurrency-and-servers program — threads, stack switching, scoped arenas,
HTTP/1.1 and h2 with TLS and WebSockets, actors, the test doubles, and four
flag-days — is complete at `0c66339d`. This section is the end state in one
place: what the suite costs to run, what the toolchain costs to ship, and the
two columns §1 promised and §6 never had.

**The suite, and what it costs to take.** All timings on the machine of §6.

| | |
|---|---|
| bench sources | 4 files, 3,654 lines of Rust (§5) |
| profiles | 20, six realistic and fourteen stress (§4) |
| checked-in corpora | 8, 550 KiB of a 2,048 KiB cap |
| digest-pinned manifests | 40, 15,546 bytes, **all forty still match** |
| `cargo nextest run -p buri` | **1,157 tests, 0 skipped**, 2 min 3 s to 8 min 38 s of test execution |
| `--validate --quick` | 0.4 s, the CI gate |
| `--validate` | 12.8 s, the saved half and the anchor's two digests |
| `--validate --set=scale` | 27.3 s, the sample's digests |
| `--validate --set=scale-full` | 4 min 20 s, all forty |
| a default run | 150 s and 153 s on two runs, every row of §6.1 |

Six of those rows are wall times and they carry one caveat between them. **This
machine was never idle** — one-minute load averages between 9 and 208 on ten
cores — so each is the fastest of the runs taken and is an upper bound. The
suite's own spread says how much that matters: four runs of the same 1,157 tests
read **2 min 3 s**, 2 min 30 s, 2 min 42 s and **8 min 38 s** of test execution.

**One test was over half the suite's wall time, and the two rows above are the
last ones taken before it was fixed.**
`buri::recovery a_syntax_error_does_not_become_a_type_error` read 68.8 s of the
fastest run's 123.0 s and once ran past `nextest`'s 300-second slow timeout. It
was one serial loop of five thousand seven hundred `analyze_snippet` calls, and
`analyze_snippet` builds a `SourceMap` and a parse cache from nothing, so each
two-hundred-byte mutated snippet re-parsed the whole standard library.
`cli/tests/recovery.rs` now computes every per-file baseline up front, fans the
cases out over `buri::parallel::map_with`, and gives each worker one `SourceMap`
and one parse cache to keep, with verdicts folded into the report in index order
on one thread. **Nothing about the population, the ceilings or the table
changed.** On the machine of §6 the whole `recovery` suite went from **77.9 s to
9.0 s**, and this test from 65.1 s to 8.9 s. CI's four-core runner read 162.9 s
for the suite and nobody re-measured it here.

The row worth pausing on is the digests. Forty pinned manifests were re-pinned
at generator revision 7 and **all forty regenerate to their recorded SHA-256 at
`0c66339d`** — `--validate --set=scale-full`, exit 0 — which is the whole
program's worth of language change landing without moving a byte the generator
emits.

**What the toolchain ships, per platform.** The runtime archive is
`include_bytes!`d into every `buri` binary, so its size is the toolchain's size,
and `cli/tests/ci.rs::the_runtime_archive_is_real` is the ratchet that holds it.
Its numbers, and the one this tree reproduced on 2026-09-01:

| triple | `net` off | `net` | `net-h3` | budget | headroom |
|---|---:|---:|---:|---:|---:|
| `aarch64-apple-darwin` | 6,329,104 | **9,097,192** | 9,097,432 | 9,437,184 | 3.6% |
| `aarch64-unknown-linux-musl` | — | **13,938,046** | — | 14,680,064 | 5.05% |

The Darwin figure does not come from the script: a `cargo build --release -p
buri` in this worktree produced an archive of exactly 9,097,192 bytes, and the
script passed over it. The Linux figure is the script's own, measured in the
Linux container BUILD-AND-WATCH.md §3.3.1 describes, and it now belongs to the
**musl** triple — 13,938,046 bytes against 13,799,068 for the `gnu` triple it
replaced, 1.0% larger, which is musl's standard library rather than anything
this repository did. It is 1.53× Darwin's for the same code, ELF's price per
byte. Both budgets are ratchets and **neither is hit**. Darwin's 3.6% is the
thinner of the two, and the script says in capitals that the next slice to add
anything at all is the one that re-measures it.

**What a phase allocates, which is the one figure with no noise in it.**
`--alloc` needs the counting global allocator (`--features alloc-counter`), so a
timed row and an allocation row never come from the same binary. On `mixed`,
2026-09-01:

| Phase | 1k | 10k | 100k | per token at 100k |
|---|---:|---:|---:|---:|
| `lex` | 529.9 | 503.8 | **503.8** | 0.073 |
| `lex+parse` | 1,261.6 | 1,179.0 | **1,174.9** | 0.169 |
| `sema` | 29,616.1 | 15,711.8 | **14,163.0** | 2.043 |

Allocations per 1,000 lines; 50,925, 118,749 and 1,431,514 allocations
respectively over the 101,074-line corpus. Two things fall out of it. **Carbon's
"no allocation per token" constraint (§1) is met with room**: the lexer
allocates once per 13.7 tokens and the parser once per 5.9, and the front end's
two rows are *flat* from 10k to 100k, so its allocation count is linear in the
program with no cliff between the two scales. And `sema`'s row is the prelude
floor seen from the other side: 29,616 per 1,000 lines at 1k against 14,163 at
100k, converging down exactly as §3.1's floor argument predicts a fixed cost
must.

**Peak memory, the fourth column §1 keeps naming.** `--rss` is an untimed
subprocess pass, one process per phase, so each row is the cost of everything up
to and including that phase. `mixed`, 2026-09-01:

| Phase | 1k | 10k | 100k | bytes/line at 100k |
|---|---:|---:|---:|---:|
| corpus in memory | 4.0 MB | 5.0 MB | 12.2 MB | 126 |
| `lex` | 4.0 MB | 6.4 MB | 29.1 MB | 302 |
| `lex+parse` | 4.2 MB | 7.4 MB | 38.0 MB | 394 |
| `sema` | 7.4 MB | 18.8 MB | 129.3 MB | 1,342 |
| `lower+js` | 10.8 MB | 36.7 MB | 289.9 MB | 3,008 |
| `lower+macos-arm64` | 33.6 MB | 60.2 MB | 318.7 MB | 3,306 |

**A hundred thousand lines peaks at 319 MB**, and the shape of the climb is the
argument for measuring it: the front end is 394 bytes a line, semantic analysis
triples that, and lowering triples it again. The three *native* triples agree to
within 2.4% of each other at 100k — 315.9 MB for `linux-x86_64`, 318.7 for
`macos-arm64`, 323.5 for `linux-arm64` — and the JavaScript row is 9% under
them, so the bulk of this is the compiler's own working set rather than any one
backend's. There is still no *goal* here, but the column is no longer empty and
a future budget has a number to be stated against.

**The runtime's cost is measured beside this rather than in it**: §6.6 prices
the shared-reference-counting fork and §6.7 prices a scope, both from the same
`allocs` program, and nothing in this slice touched the runtime.

### 6.9 Where a real repository's `buri test` goes, 2026-09-03

A maintainer's own monorepo — 18 packages, 177 `.buri` files, 173 codegen
units, 488 test cases — ran `buri test //...` cold in **8.2 s**. The question
was which stencils the run side was missing. **The run side is 0.15 s of it.**
The batched test binary the suite builds executes every block in 154 ms on this
machine. The other eight seconds are the compile, and this section is what was
in them.

The phase column comes from a timer compiled into a throwaway toolchain build,
with `buri clean` before every run and the minimum of several runs per phase.
The wall row is the two *shipping* toolchains, alternating, and it is the number
to read as the result.

| Phase | before | after |
|---|---:|---:|
| front end (parse, check, monomorphize; batched over 13 suites) | 0.25 s | 0.24 s |
| `actions::prepare` — `middle::run`, derives, fuse, closures, `rc::run` | 1.40 s | 1.40 s |
| `lower::run_with`, for the unit keys | 0.24 s | 0.24 s |
| `unit_hashes` (parallel) | 0.26 s | 0.26 s |
| **`Backend::emit_units`** | **4.06 s** | **1.19 s** |
| — of which a second `lower::run` (`rc::analyze` + `run_with` again) | 1.01 s | — |
| — of which `frame_sigs` | 0.07 s | 0.05 s |
| — of which the per-unit emission | 2.83 s, one thread | 1.10 s, ten |
| link | 0.40 s | 0.40 s |
| running the suite | 1.16 s | 1.24 s |
| **wall, cold, best of seven A/B pairs** | **7.99 s** | **5.13 s** |

`design/native/CODEGEN-STENCIL.md` §4.2 says what changed on each line. Four
findings, and only one of them is about threads:

1. **The compiler lowered the program twice.** `objects_named` lowers for the
   unit keys and `emit_units` lowered again for the bytes; each lowering carries
   a whole-program `middle::rc::analyze`. Handing the first lowering over
   deleted 1.01 s.
2. **`Cycles` got rebuilt per unit** — a walk of every constructor plus a Tarjan
   pass, 173 times. §6.4's first finding is exactly this shape, and
   `Layouts::with_cycles` exists for exactly this reason. The LLVM backend used
   it; the stencil backend did not.
3. **The emitter copied a `Layout` per instruction and cloned a `Ty` per
   reference operation**, where `Layouts::shared` says in its own doc comment
   that a caller in a loop over instructions must use it. `walk_rc` is that loop
   and used the copying form; so did `MakeStruct`, `GetField`, `GetPayload`,
   `MakeEnum` and `GetTag`. On the critical-path unit this was worth 30%.
4. **Three `format!`s and three hash lookups per emitted machine
   instruction**, for the folded twins `Jit::emit` asks about by name.

**On the bench, `lower+macos-arm64` halves.** §6.6's protocol: `--only=mixed
--set=native --targets=macos-arm64`, A/B/A/B, each cell the better of that
compiler's two run medians.

| corpus | before | after | Δ |
|---|---:|---:|---:|
| `mixed/10k` | 62.8 ms | 29.6 ms | −52.9% |
| `mixed-many-files/10k` | 48.0 ms | 20.7 ms | −56.9% |
| `mixed-few-files/10k` | 65.1 ms | 45.3 ms | −30.3% |
| `mixed-libs/10k` | 60.2 ms | 29.0 ms | −51.8% |
| `mixed-deep-graph/10k` | 62.7 ms | 29.9 ms | −52.3% |
| `mixed-wide-graph/10k` | 61.2 ms | 29.3 ms | −52.1% |
| **median** | | | **−52.2%** |

Five of the six move together, and the sixth is the finding rather than an
outlier: `mixed-few-files` is the corpus with the fewest codegen units, so it
is the one with the least to spread over cores, and −30.3% is what the three
serial findings are worth on their own.

**This section deliberately does not restate §6.1's goal-3 rate from these
numbers.** The table above is a *ratio* between two toolchains measured against
each other in one window. A goal row is an absolute rate, stated over a wider
corpus set than `--only=mixed`, and it moves when somebody takes it the way §2
says to.

**What the ceiling is now.** The per-unit emission is parallel, so it is bounded
by the **largest single unit** — on this repository `core/orderedmap`, 11,267
monomorphized functions, which is 1.10 s of the 1.10 s. Splitting a unit is a
build-system question (a unit is a cache key and an object file,
`design/native/ARCHITECTURE.md` §5), so the next win on that line is either
making a function cheaper to emit again or making a unit smaller. **§6.10 takes
that ceiling down**, by a third route neither of those names: it splits a unit's
*emission* rather than the unit. Above it, `actions::prepare` is now the largest
single-threaded phase at 1.40 s, of which 0.79 s is `middle::rc::analyze` —
still the whole-program analysis the emitter no longer duplicates, so halving it
would be worth as much again.

**Two things this did not touch.** The suite's `link` is 0.40 s for a **102 MB**
batched debug binary, written to the artifact cache and then to disk; and
`commands/test.rs`'s `run_blocks` re-`exec`s that binary once per failing block,
which on this repository is four spawns of 102 MB for three failures.

### 6.10 The largest unit stops being the ceiling, 2026-09-04

§6.9 left the emission bounded by one unit, and named two ways past it: make a
function cheaper, or make a unit smaller. There is a third that does not touch
the build system — **divide the emission of a unit without dividing the unit**.
`design/native/CODEGEN-STENCIL.md` §4.3 is the mechanism; this is the
measurement.

**The measurement is on a synthetic, and that is a caveat rather than a
footnote.** §6.9's repository no longer compiles against this toolchain: a later
change split the filesystem effect into `FileSystemRead` and `FileSystemWrite` (`core/fs`), and
nine of its eighteen packages are written against the un-split one. A program
*shaped* like the finding replaces it — one module instantiating `core/orderedmap`
at two hundred key and value types, which puts **14,200 monomorphized functions
in `core_ordmap`** against the real repository's 11,267 — so read the numbers
below as that shape rather than as that repository.

**Where the 0.61 s in the biggest unit went**, from a timer compiled into a
throwaway toolchain, minimum of several runs:

| Within one unit's emission | ms | share |
|---|---:|---:|
| the members' bodies (`Jit::compile_part`'s loop) | 405 | 66% |
| the `codegen` key's **text** (`render_func` per member) | 100 | 16% |
| the `codegen` key's **digest**, and the `String` the old render allocated per function | 64 | 10% |
| the symbol table and the relocation list | 19 | 3% |
| the Mach-O writer | 17 | 3% |
| the generated glue — 2,801 helpers | 8 | 1% |
| `Jit::plan` and `Jit::resolve` | 1 | <1% |
| **total** | **614** | |

Two thirds of it is the per-function loop, which is the ideal shape: many small
functions, no shared mutable state that is not a memo. So the emitter cuts the
members into contiguous parts of 512 and puts the parts of *every* unit on one
flat work list, leaving the per-unit assembly where it was.

| | before | after |
|---|---:|---:|
| **the whole emission** (throwaway timer, min of 5 alternating) | **585 ms** | **211 ms** |
| the biggest unit's assembly, which is now the floor | — | 67 ms |
| cold `buri build //...`, two *shipping* toolchains, A/B/A/B ×6, min | **2,022 ms** | **1,565 ms** |
| the same, medians | 2,058 ms | 1,631 ms |
| object bytes in the action cache | 96,680 KB | 97,732 KB |

**0.36× on the phase and 0.77× on the wall**, and the gap between those two
numbers is the point: the emission was 29% of this build and is now 13% of it,
so the next thing in the way is somewhere else.

**The division costs 1.1% of object bytes, and that is not free.** Three things
are per-`Jit` and become per-part — the constant pool's deduplication, the map
of a stencil's spilled constants, and the generated glue — and the glue is the
one that shows: two parts that both drop a `[Str]` get a copy each under
different local names. Across part sizes from 256 to 2048 the emission wall
stays the same to within the noise while the object bytes move about 0.2% per
halving, so 512 is the smallest part that costs about one per cent. (§6.12
made drop and copy glue weak, so the linker now keeps one copy per program.)

**On the bench, the corpus §6.9 could not help is the one that moves.** §6.6's
protocol: `--only=mixed --set=native --targets=macos-arm64 --json`, A/B/A/B,
each cell the better of that compiler's two run medians.

| corpus | before | after | Δ |
|---|---:|---:|---:|
| `mixed/10k` | 27.63 ms | 26.65 ms | −3.5% |
| `mixed-many-files/10k` | 19.60 ms | 19.66 ms | +0.3% |
| **`mixed-few-files/10k`** | **40.16 ms** | **33.63 ms** | **−16.3%** |
| `mixed-libs/10k` | 27.20 ms | 26.37 ms | −3.1% |
| `mixed-deep-graph/10k` | 28.15 ms | 27.74 ms | −1.4% |
| `mixed-wide-graph/10k` | 27.08 ms | 26.50 ms | −2.1% |
| **median** | | | **−2.6%** |

`mixed-few-files` is **the same row §6.9 singled out as the finding rather than
the outlier**: it has the fewest codegen units, so it had the least for a
per-*unit* pool to spread over and gained least there — −30.3% where the other
five halved. Dividing the inside of a unit is precisely what reaches it, and it
gains six times what any other corpus here does. The controls agree that this is
the change and not the afternoon: `lex` moved a median −0.4% over the same six
corpora, `lex+parse` −0.2%, `sema` −0.1%. One `sema` cell read +9.4%, which is
what this machine's drift looks like; read the rows from the medians.

**What the floor is now, and it is a different shape.** 67 ms of the biggest
unit is its assembly: 27 ms in the object writer, 22 ms building a symbol table
and 402,000 relocations, 13 ms concatenating the parts, 5 ms hashing. Every one
of those is one unit's own and serial by construction.

**One finding is recorded and deliberately left alone.** The stencil backend
computes a `codegen` key — 164 ms of the 614 — and then **throws it away**.
`build::actions::codegen_units_for` matches the emitted object by *name* and
keeps the key it was already handed, which `unit_hashes` computed in parallel
above the emission from the same `render_func` text. The larger half of the
164 ms is recovered here by rendering in the parts — the unit's text is those
texts concatenated, so the digest is unchanged — and the remaining 64 ms would
need `Backend::emit`'s contract to say that the key is the caller's, a
build-system change. `llvm/mod.rs` computes the same discarded key the same way,
so it is one finding and not this backend's.

### 6.11 The ownership fixpoint stops being whole-program, 2026-09-11

§6.9 named `actions::prepare` the largest single-threaded phase, of which
`middle::rc::analyze` was the half worth as much again. Two things in it were
whole-program work done the slow way, and neither had to be.

**The ownership fixpoint iterated over every function until nothing moved.**
`infer_ownership` promotes a parameter from borrowed to owned, and a promotion
propagates one call edge per pass, so a whole-program loop is Θ(functions × call
depth) — a deep program walked whole, many times. The promotion is monotone, so
the answer is a least fixed point independent of order; the loop now walks the
call graph's strongly connected components in reverse topological order, reusing
the Tarjan `middle::run` already has. Each component converges against the
already-final rows of the components it calls, and a non-recursive singleton —
almost every function — settles in one pass rather than in as many as the call
graph is deep.

**The per-function scans ran one at a time.** `rc::analyze`'s plan-building scan
and `lower::run_with`'s lowering are each a pure function of one function and
the whole-program answers above, with no cross-function shared state — the type
interner was `lower`'s only exception, and it is now a per-worker table folded
back together in index order, so the emitted type table and every id in it stay
byte-for-byte the serial ones. Both loops go through `crate::parallel::map`,
which returns its results in index order, so the output does not depend on how
the work was divided. Build output is still compared byte for byte against a
`--force` rebuild across the four edit classes finding 5 names, and the whole
golden and agreement corpus is unmoved.

Isolated, min of several runs, `mixed/100k`, macos-arm64 — a throwaway timer
compiled into the bench, the same instrument §6.9 used, each phase built once
and timed on its own rather than by subtracting cumulative rows:

| Phase | before | after | |
|---|---:|---:|---:|
| `middle::rc::analyze` (ownership SCC walk + parallel scan) | 76.90 ms | 64.16 ms | 0.83× |
| `lower::run_with` (parallel per-function lowering) | 40.55 ms | 27.98 ms | 0.69× |

The lower row is the cleaner read of the two — lowering is the larger share of
its phase that parallelizes, where `rc::analyze` still carries the effect and
parking fixpoints single-threaded beside the scan, so its 0.83× is the scan and
the SCC walk against those. These are separate binaries measured against each
other on a shared machine rather than an A/B in one window, so read the
direction and the ratio rather than the millisecond; both move the way removing
redundant passes and spreading the rest over ten cores predicts, and the
observable compiler output does not change at all.

---

### 6.12 One instance per list of bindings, 2026-10-03

Every `context { ... }` expression mints its own `CtxTypeId`, so a generic over
`C: Allocator` was compiled once per test that built a context. In
`//libs/database/server/server`, 126 contexts made 127 context types, and all
but one had the same bindings. `OrderedMap.insert` alone had 2,676 instances.

Monomorphization now rewrites each context type to the first one minted with
equal bindings, compared in order (`monomorphize.rs`'s `canonical_contexts`).
The checker doesn't change: two contexts are still two types in the language.
Symbols spell a context by its bindings, so adding a context renames nothing
and the `codegen` cache keeps hitting. Sorting the bindings as well would merge
**0** more context types anywhere in the profiled repository, so binding order
stays part of the identity.

Drop and copy glue is then named by a hash of what its body reads
(`Layouts::glue_key`) and defined weak, so the linker keeps one copy per
program: a weak private external on Mach-O, a COMDAT group on ELF, and
`linkonce_odr` on LLVM.

A copy of the profiled monorepo, release toolchains, clean runs at load average
44–118:

| `//libs/database/server/server` | before | contexts merged | and glue shared |
|---|---:|---:|---:|
| test binary | 851 MB | 15.8 MB | **12.9 MB** |
| objects in the link directory | 904 MB | 32 MB | 32 MB |
| `.buri/cache` | 1.7 GB | 32 MB | 30 MB |
| relink, `ld64.lld` / Apple `ld` | 2.5–4.5 s / 3.0 s | 0.19 s / 0.21 s | 0.29 s / 0.34 s |
| clean `buri test`, CPU (user + sys) | 38.7–42.5 s | 6.1 s | 6.7–7.0 s |
| clean `buri test`, wall | 30–110 s | 11.4 s | 7.2–16.8 s |

| `buri test //...` (1,090 tests) | before | after |
|---|---:|---:|
| CPU (user + sys) | 111.4 s | 40.7 s |
| wall | 328.6 s | 118.0 s |
| `.buri/cache` | 3.4 GB | 310 MB |
| largest test binary | 851 MB | 47.6 MB |

Wall time at this load moves by 2–4× between runs of one binary; CPU time is
the number to compare. Shared glue takes the server binary another 18% down and
leaves CPU time inside the noise.

### 6.13 Front ends on the pool, 2026-10-03

`buri test` used to type-check and monomorphize every suite on the main thread,
because syntax trees, the source map and the parse cache were `Rc`. They are
`Arc` now. The main thread still loads each suite, so file ids keep a fixed
order. A worker then checks it, monomorphizes it, and goes straight on to build
and run it:

```text
main thread   plan → load suite 1 → load suite 2 → …        → report in label order
worker        check 1 → monomorphize 1 → link 1 → run 1
worker                   check 2 → monomorphize 2 → link 2 → run 2
```

One standard library snapshot serves the whole process instead of one per
thread. Workers get the source map as one shared copy (`SourceMap::shared`),
taken again only after the map grows. Analyses the lint never reads are freed
off the main thread.

A front end and its back end are one heavy job, so the memory cap now covers
front ends too: at most `builds_of` suites hold a whole analysis or program at
once. The pool claims that room when a worker takes a heavy job, not when it's
queued. Queueing never waits, so a batch can queue its groups while it holds
its own.

The single-threaded cost of `Arc` is inside the noise. Interleaved runs of
both bench binaries at load average 30–70, best of the rounds:

| row | before | after |
|---|---:|---:|
| `mixed/10k` sema | 10.50 ms | 9.64 ms |
| `mixed/100k` sema | 88.17 ms | 84.01 ms |
| `mixed/100k` lex+parse | 21.18 ms | 21.02 ms |
| `struct-heavy/10k` sema, five rounds | 9.03 ms | 8.55 ms |
| the snapshot, once | 4.3–4.6 ms | 4.2–4.6 ms |

The profiled monorepo's copy (1,768 tests; 12 suites fail to compile in the
copy itself), release toolchains, two interleaved rounds at load average 20–88:

| `buri test //...` | before | after |
|---|---:|---:|
| cold, wall | 17.8–20.8 s | 17.8–20.0 s |
| cold, CPU (user) | 47.0–47.5 s | 47.1–49.7 s |
| cold, peak memory | 1,293–1,299 MB | 1,368–1,393 MB |
| warm, wall | 1.20–1.25 s | 0.72–0.75 s |
| warm, peak memory | 147 MB | 282–291 MB |
| a body edit in `//libs/ui/theme`, wall | 5.39–5.68 s | 5.31–5.32 s |
| a body edit, peak memory | 891 MB | 891 MB |

Output is byte-identical, diagnostics included. A warm pass still re-checks
the 12 broken suites, since only a clean run is cached; those now check four at
a time, which is both the speedup and the extra peak memory. A cold pass isn't
front-end bound on this repository: the profile puts its long poles in building
the generator tools while the session opens and in the test processes
themselves. With room
for ten builds (`BURI_TEST_MEMORY_BYTES`) the edit pass drops to 4.9–5.2 s and
cold stays where it is.

So the budget is now bytes, not builds. Run one at a time, a suite's build adds
25–440 MB to the peak, and the batch of 58 suites peaks at 1.9 GB for the whole
run. Each build is queued with `64 MB + 400 × its repository source bytes`, an
upper bound on every one of those, and the builds in flight share half the
machine's memory. On 32 GB that is room for every worker.

The generator tools weren't the cold pass's long pole either: building all
five took 0.6 s. Running them did. The session asked the `proto` check about 33
schemas one process at a time, then ran each rule one after another, for 6.1 s
of `open_at` under load. Rules now run in rounds, side by side, with a rule's
checks side by side too, and the same `open_at` takes 1.9 s.

### 6.14 Lex+parse breaks its plateau, 2026-10-03

`mixed/100k`, `main` at `2bc82d3f` against `perf/lex-parse`, alternated
A/B/A/B/A/B on one machine at load 21–25. The first `main` leg read ±36.7% and
went in the bin.

| | `main` | after | Δ in rate |
|---|---:|---:|---:|
| lex | 7.93 M lines/s | **14.77 M** | **+86%** |
| lex+parse | 4.77 M lines/s | **7.44 M** | **+56%** |
| lex+parse, fastest sample | 21.03 ms | 12.89 ms | |
| lex peak RSS (`--rss`) | 29.4 MB | 28.0 MB | −4.8% |
| lex+parse peak RSS | 38.9 MB | 37.7 MB | −3.1% |
| token buffer | 13 B/token | 12 B/token | |

This machine read 6.36 M lines/s for `main`'s ancestor on a quiet day and
4.77 M today, so 7.44 M is 1.34× short of the goal here and about at it on the
quiet day's scale. Somebody should re-take §6.1 on a quiet machine.

**The lexer was the bigger half, and branches were its cost.** A profile put
`lex` at 54% of the phase, and allocation at 13%. Removing every doc comment's
`String` saved 3%, so allocation was not the lever. Each change below is
`cli/benches/corpora/mixed-10k` through `parse`, the fastest of 2 s of
repetitions, alternated three or four times:

| Change | lex | lex+parse | load |
|---|---:|---:|---:|
| Keywords through a perfect hash: one multiply, one load, one compare | −40% | −24% | 65 |
| The `expect` family, `enter` and `link` inline their right path, and call a cold function to report | | −8.5% | 38 |
| One `match` per token: trivia, literals, words and punctuators share one dispatch | −5.8% | −4.9% | 35 |
| A run of blanks stepped over in one loop | −3.7% | −1.5% | ~100 |
| One 12-byte record per token instead of three columns | −2% | ±0 | 55 |
| A word's key read in one 8-byte load | −1% | −1.5% | 17 |
| Operator table keyed on `TokenKind`; `postfix_ops` checks for an operator first | | −1% | 24 |
| Arenas sized from the token count, at what `mixed` writes per token plus a tenth | | ±0 | 54 |

The keyword change is out of all proportion to the `memcmp` it removed, which
was 8% of the profile. The old `match` on a `&str` was a length switch and a
chain of compares, so most of the gain is mispredictions that went with it.

Arena sizing bought no time, and cut reallocations from 231 to 144 per 1,000
lines. Its first version reserved half again what a file needs and raised the
lex+parse peak to 41.3 MB; the per-arena figures fixed that.

**What is left.** The lexer is still half the phase. Allocation is 940 per
1,000 lines, and most of the parser's share is declarations: a `Box` per item,
a `Vec` per parameter list, and a `String` per doc line. Moving those into the
tree's arenas changes every consumer of the syntax tree, so it waits for a
change that can own that.

### 6.15 Analyses share the snapshot's tables, 2026-10-03

`Checker::resume` used to deep-copy the snapshot's tables, scopes, bodies and
constants into every analysis. A snippet analysis copies the whole checked
library. Timers around `resume` and `run` over the 7,000 analyses of
`a_syntax_error_does_not_become_a_type_error` put **59%** of checker time in
the copy: 2.7 ms per analysis, against 1.9 ms of checking.

Now each table keeps the base's entries behind an `Arc` and the analysis's own
after them (`semantics/layered.rs`):

```rust
pub struct Layered<T> { base: Arc<[T]>, own: Vec<T> }          // fns, tycons, traits, scopes…
pub struct IdMap<I, T> { base: Arc<[Option<T>]>, own: Vec<Option<T>>, replaced: HashMap<usize, T> }
pub struct LayeredMap<K, V> { base: Arc<HashMap<K, V>>, own: HashMap<K, V> }  // impls
```

Ids keep running after the base's, so an id means the same entry in either
half. Resuming costs a reference count per table. `IdMap` holds bodies,
constants, the method table and each type's traits: one slot per id, walked in
id order. `replaced` holds the base entries an analysis rewrites, such as
bodies style extraction rewrites, or a method added to a base type.

Each change below is measured A/B against the one before. "Instructions" is the
recovery test's `instructions retired` from `/usr/bin/time -l`: it doesn't move
with load, and the load stayed between 20 and 110 all afternoon. Allocations
are `--alloc` on `sema`.

| Change | Recovery test: instructions, CPU | `mixed-1k` / `mixed-10k` allocations |
|---|---:|---:|
| `main` | 175.8 G, 25.4 s | 63,850 / 194,362 |
| Layered tables instead of the copy | 42.2 G, 5.6 s | |
| Passes after checking start at the base's last id; style extraction skips anything with no style in it | 35.4 G, 3.2 s | 54,742 / 185,258 |
| Resolve a body in place; locals in one flat list of name hashes; no copy of the `FnInfo` per body | 30.1 G, 3.3 s | 44,853 / 153,474 |
| Unify borrows what no variable stood for; literal checks borrow their spelling | | 40,622 / 144,644 |
| The base keeps the prelude; known names are recorded once | 29.0 G, 3.1 s | |
| Methods and conformances in an `IdMap` | 28.6 G, 3.1 s | 40,578 / 144,601 |

The first two rows ran before `main`'s lex+parse and driver changes merged
in. Those took the starting point from 179.0 G to 175.8 G.

| End to end, `main` against all of it | before | after |
|---|---:|---:|
| recovery test, wall | 4.0–4.6 s | 0.56 s |
| recovery test, peak memory | 323–327 MB | 242 MB |
| `sema` floor | 2.89–3.02 ms | 2.01–2.08 ms |
| `mixed/1k` sema | 3.77–4.03 ms | 2.83 ms |
| the snapshot, once | 4.7–5.1 ms | 3.8–3.9 ms |
| `saved:mixed-10k` sema peak RSS (`--rss`) | 27.5 MB | 26.3 MB |
| cold `buri test //...` on the monorepo copy, wall / CPU | 48–50 s / 127 s | 47–49 s / 125–128 s |
| cold `buri test //...`, peak memory | 2,042–2,067 MB | 2,006–2,034 MB |

A cold `buri test` doesn't move, which matches §6.13: it isn't front-end bound.
Its JavaScript and generator outputs are byte-identical once each test
library's `$t.seed` is masked; the seed hashes the toolchain's identity, so a
different binary changes it.

The style pass got cheap because extraction only rewrites a style or a list of
them. A body with neither is left alone, so it stays shared instead of being
copied out of the map, and the base stops handing every constant back to each
analysis.

**What's left.** Three quarters of what checking still allocates is building
the typed tree and copying `Ty`s. `Ty` is 40 bytes, and the commonest
allocation is one 40-byte `Ty` behind a `Box` or in a one-element `Vec`.
Interning types to `u32` ids would remove most of it, and so would boxing
`Ty::Fn`'s two fields, which takes `Ty` to 32 bytes. Both change a type the
middle end and every backend match on, so they wait for a change that owns
those too.

Native objects aren't reproducible across two cold runs of one `main` binary
in this repository: same cache key, different bytes. That is why the native
half of the output wasn't compared above.

### 6.16 Declarations move into the tree's arenas, 2026-10-03

§6.14 left the parser allocating per declaration. Now nothing in a declaration
owns heap memory:

```rust
pub struct FnDecl { name: Name, generics: List<GenericParam>, params: List<Param>, docs: Docs, .. }
pub enum Item { Fn(FnDecl), Struct(StructDecl), .. }   // inline, 72 bytes; was Fn(Box<FnDecl>)
for p in tree.list(d.params) { .. }                     // was `for p in &d.params`
tree.doc_lines(d.docs)                                  // was `d.docs: Vec<String>`
```

- `module.items` is the item arena: one `Vec` per file instead of a `Box` per declaration.
- Parameters, generics, fields, tuple fields, variants, methods and import names
  are `List<T>` ranges into arenas on `flat::Tree`. `Tree::list` picks the arena by `T`.
- A doc line is the location of its text. The lexer writes every `///` line into
  one `Vec<Location>`, and the tree adopts it whole.

Every consumer reads through `tree.list` and `tree.doc_lines`: the checker, the
formatter, the language server, `buri docs`, lint, tool contracts and the
module loader. A cold build of all 365 repositories under `cli/tests/repositories`
prints the same bytes on both binaries, and the worked example builds identical
native objects.

`main` at `828a8361` against `perf/ast-arenas`, alternated A/B three times. The
timing rounds ran at load 26–34, `--rss` at 62–81 and the repositories at 78–87.
Times are the fastest sample.

| | `main` | after | Δ |
|---|---:|---:|---:|
| `mixed/100k` lex | 6.73–6.75 ms | 5.99–6.19 ms | −10% |
| `mixed/100k` lex+parse | 13.25–13.34 ms | 11.94–12.13 ms | **−9.5%** |
| `mixed/100k` lex+parse rate | 7.60 M lines/s | **8.39 M** | +10% |
| `saved:mixed-10k` lex+parse | 1.28–1.33 ms | 1.16–1.20 ms | −9% |
| `mixed/100k` sema | 70.5–70.7 ms | 69.6–71.7 ms | flat |
| `saved:mixed-10k` lex+parse allocations | 1,055 per 1k lines | **389** | −63% |
| `saved:mixed-10k` lex allocations | 490 per 1k lines | 177 | −64% |
| `saved:mixed-1k` lex+parse allocations | 1,131 per 1k lines | 448 | −60% |
| `mixed/100k` lex+parse peak RSS | 37.5 MB | 34.1 MB | −9% |
| `mixed/100k` lex peak RSS | 27.9 MB | 25.0 MB | −10% |
| `mixed/100k` sema peak RSS | 134.1 MB | 132.0 MB | −1.6% |
| recovery test, instructions | 34.10–34.14 G | 33.88–33.97 G | −0.5% |
| recovery test, peak memory | 222–257 MB | 214–251 MB | noise |
| cold `buri build` of the 365 repositories, CPU (user) | 16.9–17.5 s | 17.4–17.6 s | flat |
| cold `buri build //...` of `cli/tests/example`, CPU (user) | 1.14–1.20 s | 1.13–1.15 s | flat |

The lexer gained the most. It used to copy each doc line into a `String` and
move the run into the trivia table; now it pushes a location. Sema doesn't move:
its allocations are 14,245 per 1k lines either way, and reading a list through
`tree.list` costs it nothing measurable. The cold builds aren't front-end bound,
as §6.13 found.

**What's left.** Lex+parse allocates 389 times per 1,000 lines. Most of that is
the lexer's trivia table, which still holds a `Vec<Comment>` and a `String` per
ordinary comment, and the cooked text of string literals, an import's path and a
test's name included.

### 6.17 JavaScript emission, 2026-10-03

JS emit was 62% of `lower+js` on `mixed/100k`, all on one thread, and 40% of
it was `malloc`. On `mixed/1k`, half of it was `collect_idents_raw` rescanning
the 340 KB runtime four times a build, with a `String` per identifier. Six
changes, each measured A/B against the one before:

| Change | Commit | What it bought |
|---|---|---|
| Each runtime declaration carries its identifiers, scanned once a process; identifier walks borrow names | `b9edfaa00` | `lower` floor 3.9 → 0.5 ms; `mixed/1k` `lower+js` 7.2–7.5 → 4.0–4.1 ms |
| `distinct` keys switch labels in a hash set | `918e5fa81` | `wide-match/40k` `lower+js` 13.2 G → 2.6 G instructions; now linear |
| Fold, clean and `switches` per top-level statement on `parallel::map`; mangling too | `c8b37efd4` | `mixed/100k` emit 268–293 → 187 ms |
| Each function generated on a worker; `merge_identical` keys a tree, not its printed text | `9dfd77111` | `mixed/100k` emit 154 ms |
| Rewrites and the simplifying constructors keep their boxes; one map of compact records in local cleanup | `f216959a6` | `mixed/100k` 4.68 G → 3.79 G instructions (−19%); emit 124 ms |
| Programs under 64 functions stay on one thread | `6904f19e9` | floor 1.1 → 0.5 ms again |

Output is byte-identical: every corpus in `--quick --set=full`, plus
`mixed/100k` and `wide-match/20k`, debug and release, diffed against `main`.

A generated function's constants are numbered on its worker from `$k0`, and
`Gen::adopt` renames them into the program's table in function order. That's
the order one generator would have numbered them in, so the names don't move.

| End to end | before | after |
|---|---:|---:|
| `mixed/1k` `lower+js`, fastest sample | 7.2–7.5 ms | 3.5–3.7 ms |
| `mixed/100k` `lower+js`, fastest sample | 409 ms | 246 ms |
| `mixed/100k` emit, split | 273 ms | 124–129 ms |
| `mixed/100k` peak RSS after `lower+js` | 307 MB | 305 MB |

Those rows ran at load 10–16. `wide-match` ran at load 50–110, so its
scaling is read off instructions retired (`/usr/bin/time -l`, a `lower+js`
child minus a `sema` child), which don't move with load:

| `wide-match` lines | before | after | peak RSS before / after |
|---:|---:|---:|---:|
| 2.5k | 284 M | 165 M | 30 / 28 MB |
| 5k | 570 M | 272 M | 44 / 43 MB |
| 10k | 1,385 M | 489 M | 73 / 70 MB |
| 20k | 4,013 M | 915 M | 135 / 117 MB |
| 40k | 13,245 M | 1,791 M | 245 / 231 MB |

Each doubling now costs 1.7–1.9×, where it cost 2.4–3.3×.

**What's left.** On `mixed/100k` the serial part of emission is
`rc::sharing` and the final `print`. Printing in parallel needs `emit_with`
to ask for it, since `print` also runs on case bodies inside the workers.
`Expr` is 56 bytes because `ArrowBlock` holds two `Vec`s inline, and
`Stmt::If` is about 104; boxing those payloads shrinks every node, but
`crossing.rs` builds them too.

### 6.18 Type checker hot spots, 2026-10-03

A profile of `mixed/100k` ranked five spots in `semantics/`. Each change below
is one commit, measured against the one before it. "Instructions" is a `sema`
child minus a `lex+parse` child (`--rss-child`, under `/usr/bin/time -l`),
median of three. It doesn't move with load, which ran 27–93 all evening.

| Change | Commit | `mixed/100k` instructions | `ui-style` instructions |
|---|---|---:|---:|
| `main` at `3a2a1b59` | | 1,080 M | 329 M |
| Style extraction copies only the declarations it rewrites | `c60d7846` | 1,079 M | 332 M |
| `con_carries_effect` reads the type's own trait list | `79d79f3a` | 960 M | 309 M |
| Exhaustiveness rows borrow patterns and types; witnesses only when asked | `1d275ed7` | 896 M | 298 M |
| A call reads its callee's generics in place | `93d54c50` | 887 M | 292 M |
| `unify_at` borrows the checked type | `9b4a9e70` | 882 M | 294 M |
| A small matrix scans its heads instead of building a set | `67f29956` | 875 M | 289 M |
| Three copies off the per-binding and per-call paths | `4f4b0751` | **839 M** | **282 M** |

`ui-style` is `saved:mixed-10k` with a token enum, a style constant and six
styled components added to every module: 13,231 lines that all import
`ui/style`. It isn't checked in.

The effect predicates were the biggest single cost. `con_carries_effect` asked
the impl table about each of the standard library's twenty-odd effects, two
hash probes each, at every type-constructor node the predicates walk. Now it
scans the short sorted list of traits the type implements, which `add_impl`
already keeps:

```rust
self.traits_of_con(con).iter().any(|t| self.trait_(*t).is_effect)
```

That made a memo per type unnecessary.

Exhaustiveness rows are now `Vec<&Pat>`, so specializing a row copies pointers,
not patterns. Column types are `Cow<Ty>`, borrowed wherever a step leaves the
column alone. Reachability only asks whether there is a witness, so it no
longer builds one.

Style extraction made no measurable difference. `main` already rewrote only
bodies that mention a style, so the cloned maps cost one deep copy per styled
body plus one per constant. Now the walk reads the maps untouched and rewrites a
copy of each declaration it changes.

| End to end, `main` against all of it | before | after |
|---|---:|---:|
| `mixed/100k` sema, fastest sample | 70.2–76.3 ms | **57.8 ms** (−18%) |
| `mixed/1k` sema, fastest sample | 2.58 ms | 2.05–2.11 ms |
| `ui-style` sema, fastest sample | 17.95–18.25 ms | 15.69–16.02 ms |
| `wide-match/40k` sema instructions | 550–555 M | 476–480 M |
| `mixed/100k` sema allocations, per 1,000 lines | 10,213 | 9,418 (−7.8%) |
| `ui-style` sema allocations, per 1,000 lines | 22,051 | 20,626 |
| `mixed/100k` sema peak RSS | 130–131 MB | 130–132 MB |
| `ui-style` sema peak RSS | 37 MB | 37 MB |

Timed rows are two alternating runs each, at load 34–93, so they're read off
the fastest sample. Diagnostics, goldens and generated code don't change: the
full suite passes unmodified.

**What's left.** A sampling allocator (every 101st allocation's backtrace) puts
most of what checking still allocates in `Ty`'s derived `Clone`: a `Vec<Ty>` or a
`Box<Ty>` copied for a pattern's type, a call's substituted parameters, or a
local's type at each use. Interning `Ty` removes those. The next largest are the
`String`s in `ParamInfo` and `Local` names.

### 6.19 Native emission and the middle end, 2026-10-04

A cold native `buri test //...` on a 290 MB monorepo spent 30% of the `buri`
process's CPU in stencil emission and 11% in `prepare`. Five changes, all
output-identical:

| Change | Commit | Monorepo CPU before → after |
|---|---|---:|
| The stencil backend stops rendering the `codegen` key that §6.10 found it throws away; `Emitted::key` is `Option` and the build system's. The renderer writes into its buffer instead of a `format!` per operand | `ec75b5306` | `render_func_into` 1.15 → 0.08 s; `unit_hashes` 0.73 → 0.27 s |
| The stencil tables use `hash::Map`; the `+swap` twin resolves by index next to the fold twins, and `arm_key` borrows | `ef7b18337` | `Library::at` 0.19 → under 0.02 s; `arm_key` 0.12 → 0.05 s |
| `inline::run` measures each body once and again only after it changes; later rounds revisit only functions whose body changed or whose callees' facts moved; a body with nothing foldable skips its fold | `d81ba5f42`, `78ec433ef`, `658b5c3f0` | 0.89 → 0.73 s |
| `decision::rewrite` decides a match from borrowed patterns, then moves its arms | `af4c60008` | 0.36 → 0.07 s |
| `Jit::promote` filters back edges through a dominator tree built once, and reuses one stamped table. `Term::targets` is an iterator, and `Term::target(k)` fixes the quadratic layout walks in the stencil and LLVM backends. Switch lowering finds repeated variants with a set | `492642a08` | `wide-match` is linear, below |

Each row is the inclusive time in one `samply` profile per side, at load
60–90. The whole `buri` process went from 14.5 s to 11.6 s of CPU: emission
4.29 → 3.08 s, unit keys 1.70 → 1.14 s, `prepare` 1.60 → 1.18 s. Wall time
at that load moved by more than the change, so the A/B that holds is
instructions retired by the whole run, test binaries and linker included:
116.0–116.5 G before, 95.8–96.5 G after, two alternating runs each (−17%).

Every object the run links is byte-identical to `main`'s: 889 objects,
compared by SHA-256.

**Parallel inlining was tried and dropped.** A round's answer depends on
index order, since a caller pastes its callee's current body. Cutting the
round into order-respecting levels kept the output identical, but starting
the pool once per level cost more than the work. On `mixed/10k`, `middle-A`
went from 5.3 ms to 8.4 ms. `buri test` already prepares one program per job
thread.

The bench's native rows, best of five alternating runs at load 5–60:

| corpus | before | after | Δ |
|---|---:|---:|---:|
| `mixed/10k` | 24.95 ms | 22.24 ms | −10.9% |
| `mixed-many-files/10k` | 17.30 ms | 16.50 ms | −4.6% |
| `mixed-few-files/10k` | 31.25 ms | 27.51 ms | −12.0% |
| `mixed-libs/10k` | 24.20 ms | 22.86 ms | −5.5% |
| `mixed-deep-graph/10k` | 24.88 ms | 22.78 ms | −8.4% |
| `mixed-wide-graph/10k` | 24.74 ms | 22.67 ms | −8.4% |
| `many-small-fns/10k` | 14.36 ms | 14.02 ms | −2.4% |
| `few-large-fns/10k` | 16.97 ms | 13.08 ms | −22.9% |
| `derive-heavy/10k` | 23.55 ms | 21.32 ms | −9.5% |
| `struct-heavy/10k` | 11.64 ms | 11.08 ms | −4.8% |
| `enum-heavy/10k` | 33.56 ms | 30.18 ms | −10.1% |
| `wide-match/10k` | 45.32 ms | 16.41 ms | −63.8% |
| `deep-nesting/10k` | 15.30 ms | 11.31 ms | −26.1% |
| **median** | | | **−9.5%** |

The `lower` floor didn't move: 0.46–0.48 ms before, 0.49–0.56 ms after.

`wide-match` `lower+macos-arm64`, best of two alternating sweeps at load 4–17:

| lines | before | after |
|---:|---:|---:|
| 2.5k | 6.62 ms | 4.04 ms |
| 5k | 15.28 ms | 7.99 ms |
| 10k | 46.59 ms | 15.79 ms |
| 20k | 134.78 ms | 32.45 ms |
| 40k | 440.54 ms | 75.06 ms |

Each doubling now costs 2.0–2.3×, where it cost 2.3–3.3×. The cause was
`lower` placing a match's join block ahead of its arms. Every arm's jump
looked like a back edge to `promote`, and each one walked back to the entry
with a fresh table. The layout walks listed a 20,000-case switch's targets
once per case.

**What's left.** `Jit::function` is now 21% of the process, and `inline_expr`
cloning callee bodies is most of what remains of `inline::run`. The LLVM
backend still renders the `codegen` key it hands back (`llvm/mod.rs`
`codegen_key`); the build system replaces it like the stencil one, so the
same deletion applies there.

### 6.20 Interned types, 2026-10-04

§6.15 and §6.18 left `Ty` as the biggest cost in checking and
monomorphization: a 40-byte owned tree, copied whole into every typed node,
with a `Box` or `Vec` per level. Now a type is a reference into one
process-wide table that holds each shape once (`semantics/intern.rs`):

```rust
pub struct Ty(&'static TyData);           // 8 bytes, Copy; == is a pointer compare
match ty.kind() {                         // &'static TyKind
    TyKind::Con(id, args) => …,           // args: &'static [Ty]
    TyKind::Array(elem) => …,
    …
}
Ty::con(id, args.iter().map(|a| subst.resolve(a)))   // looks the shape up, adds it if new
```

- Every phase shares the table, so `ty.kind()` works on any thread. A table
  per analysis would have to be passed to every function that reads a type.
- 64 shards behind a mutex each; reading a type takes no lock. Profiles show
  no contention, and `intern` is 0.6% of a whole `mixed-10k` run.
- Each entry carries flags for "has a variable", "has a parameter or `Self`"
  and "has `Error`". `Subst::resolve` and `substitute` return a type untouched
  when the flags say nothing in it can change, which is most calls.
- `Hash` writes a structural hash stored on the entry, never the address, and
  `Debug` prints the structure the old derive printed. So which thread interned
  a shape first can't reach output. There's no `Ord`.
- Inference is unchanged: a variable is a `Var(id)` type, bound in the body's
  `Subst`.
- `ir::TypeId` stays. It numbers one program's types for the backends, and its
  map is now keyed by interned types.
- Entries are never freed. They're bounded by distinct shapes, and every id in
  a shape is a small dense number each analysis reuses, so a language server
  re-checking a program finds its types already there.

Instructions retired and peak RSS per phase, from `--rss-child` under
`/usr/bin/time -l`, median of three alternating runs at load 33–50. Phase rows
are net of the phase before: `sema` minus `lex+parse`, the lowering rows minus
`sema`.

| corpus | phase | `main` | interned | Δ |
|---|---|---:|---:|---:|
| `saved:mixed-10k` | `sema` | 154 M | 137 M | −11% |
| | `lower+js` | 452 M | 419 M | −7.4% |
| | `lower+macos-arm64` | 740 M | 666 M | −10% |
| `mixed-100k` | `sema` | 863 M | 755 M | −12.5% |
| | `lower+js` | 3,672 M | 3,431 M | −6.6% |
| | `lower+macos-arm64` | 5,802 M | 5,210 M | −10% |
| `generic-blowup-100k` | `sema` | 928 M | 767 M | −17% |
| | `lower+js` | 5,349 M | 4,808 M | −10% |
| | `lower+macos-arm64` | 7,940 M | 6,820 M | −14% |

| End to end, `main` against interned | before | after |
|---|---:|---:|
| `mixed-100k` peak RSS after `sema` | 140 MB | 109 MB (−22%) |
| `mixed-100k` peak RSS after `lower+js` | 316 MB | 233 MB (−26%) |
| `mixed-100k` peak RSS after `lower+macos-arm64` | 391 MB | 306 MB (−22%) |
| `generic-blowup-100k` peak RSS after `lower+macos-arm64` | 495 MB | 382 MB (−23%) |
| `saved:mixed-10k` sema allocations, per 1,000 lines | 11,560 | 10,101 (−13%) |
| `saved:mixed-10k` sema, fastest sample | 6.94–8.22 ms | 5.42–5.62 ms (−22%) |
| `saved:mixed-1k` sema, fastest sample | 2.13–2.18 ms | 1.53–1.65 ms (−28%) |
| recovery test, instructions | 31.3–31.5 G | 28.0–28.1 G (−11%) |
| recovery test, CPU | 3.51–3.53 s | 3.32–3.34 s |
| recovery test, peak memory | 237–254 MB | 190–198 MB (−20%) |
| cold `buri test //...` on the monorepo copy, instructions | 94.4–94.5 G | 78.9–79.2 G (−16%) |
| cold `buri test //...`, peak memory | 2,100–2,176 MB | 1,703–1,869 MB |
| cold `buri test //...`, CPU (user) | 74–76 s | 78–79 s |

Timed lowering rows moved within noise at load 12–37, so the instruction
counts above are the reading for them. The monorepo ran two alternating pairs at
load 4–32; its wall time swung 26–112 s with the load and isn't comparable.
User CPU rose 4% while instructions fell 16%, which is the load on the
machine, not the change: the recovery test, run back to back, fell on both.

Output is identical. The full suite passes unmodified, the LLVM `native` suite
passes, including `reproducible.rs`, and all 889 objects of a cold monorepo
`buri test` match `main`'s by SHA-256. Its JavaScript matches once each test
library's `$t.seed` is masked (§6.15).

**What's left.** `Ty` no longer shows in a profile. Allocation in checking is
now the typed tree's own `Box<Expr>` and `Vec<Expr>`, and in lowering it's
`inline_expr` and `ExprKind::clone` copying bodies.

### 6.21 Where the full suite's time goes, 2026-10-05

The full suite (`cli/tests/README.md`, "The five-minute budget") took 104 s to
build cold and 378 s to run at `--test-threads 4`, at load 14–24. That run used
1,111 s of user CPU and 463 s of system CPU. It's bound by CPU rather than by
scheduling, so the only way under five minutes is less work per test.

Test time by binary:

| Binary | Test time |
|---|---:|
| `native` | 481 s |
| `build` | 307 s |
| `docs` | 252 s |
| `language` | 132 s |
| `buri-rt-tests` | 55 s |
| `fuzz` | 53 s |
| `failing` | 45 s |

The slowest tests were the three big `docs::every_manifest_id_is_fetchable`
shards at 72 s each, then `build repositories::snapshots` at 58 s and
`native cross::a_linux_x86_64_…` at 43 s.

**Half of the suite's CPU is in `buri` processes.** A run starts 4,798 of
them, costing 573 s of user CPU and 223 s of system CPU, children included. To
count them, `target/debug/buri` was swapped for a wrapper that runs the real
binary and logs its `wait4` rusage, and the suite was run from a
`cargo nextest archive` so cargo couldn't relink it:

| Command | Launches | CPU |
|---|---:|---:|
| `test` | 651 | 460 s |
| `docs` | 2,178 | 156 s |
| `build` | 1,313 | 109 s |
| `run` | 72 | 26 s |
| `lint` | 321 | 22 s |
| `lsp` | 137 | 13 s |

Starting the process is cheap: `buri version` retires 21 M instructions. The
cost is what each launch repeats. Three fixes, each output-identical:

| Change | Commit | Before → after |
|---|---|---:|
| `buri docs <id>` loads the prelude and the module the id names, not the whole library, and renders that module alone. An error, lint, command or topic id loads nothing | `fd767b29e` | `api` id 230 M → 46–52 M instructions; `error` id 234 M → 24 M; every manifest id once, 41 s → 9.5 s of user CPU |
| The stencil library writes a table of where each stencil starts, and a stencil is read when the emitter first asks for it. It used to decode all 25,000 stencils in every process | `68b920bce` | warm `buri test` on `testing/caching` 342 M → 166 M instructions; on `ui/sweep_paint_edges` 807 M → 637 M |
| The runtime archive is kept by its stamp under `~/.buri/toolchain-build/runtime/` as well as in the target directory | `f5226cf04` | a cold build's runtime step, 47–65 s → 1.25 s |

The `docs` output was compared over every manifest id, as JSON and as text,
plus misses, the index, the manifest and a search: 89,146 lines, identical.
The stencil change was checked on four native fixtures: 114 objects and
executables, identical by SHA-256.

**The runtime archive was supposed to be shared already, and wasn't.** A
`cli/build.rs` store under `<target-dir>/buri-runtime/` never served a fresh
target directory, for three reasons:

- It found the target directory by `.rustc_info.json`, which cargo writes when
  it exits, so the build that paid never stored anything. Now it uses
  `CACHEDIR.TAG`, which cargo writes when it creates the directory.
- The stamp hashed `DYLD_FALLBACK_LIBRARY_PATH`, which cargo points into the
  target directory. The target directory is now a placeholder, like `OUT_DIR`.
- The stamp hashed `nix develop`'s `out`, which names the checkout. It now sits
  in `SHELL_BOOKKEEPING`.

With the key path-independent, a new worktree, a `cargo clean` or a second
clone of a commit already built reuses the archive. Two checkouts of one commit
already produced the same digest, so the bytes served are the bytes a build
would write. A home directory that can't be written to, as in a sandboxed nix
build, builds as before.

`cargo build -p buri --profile test --bin buri` with sccache warm, each from a
fresh target directory, at load 17–29:

| | build script | whole build, wall |
|---|---:|---:|
| before | 46.8 s, or 65.0 s from a second checkout | 75.5 s, 94.7 s |
| after, from a second checkout | 1.25 s | 25.9 s |

The new critical path is `buri-stencil`'s build script, at 12–21 s, which runs
clang over the generated stencil C. The same stamp-and-store would apply to it.

**The whole bar moved less than any one row.** The same commands, `main` then
this branch, at load 7–22, with `CARGO_BUILD_JOBS=3` and `--test-threads 3`:

| | `main` | after |
|---|---:|---:|
| cold build, `nextest run --no-run`, fresh target directory | 145 s | 126 s |
| `cli/build.rs` within it | 46 s | 1.1 s |
| suite run, last alternating pair | 396 s | 391 s |
| suite run, user CPU | 1,067 s | 1,051 s |
| **total** | **541 s** | **517 s** |

At three jobs the cold build is bound by its 300 s of compile CPU, so taking
the runtime off the critical path saves 19 s rather than 45. The suite run
barely moves because the `docs` shards spend their time waiting for a seat in
the run-wide pool, not computing: alone, a shard went from 72 s to 16–18 s,
but in a full run it still takes 62–72 s. Earlier pairs at load 26–55 swung
by 60 s between runs of the same build, so they aren't quoted. Getting under
five minutes takes the linker work below.

**The linker tools are the biggest cost left, and none of it is linking.**
A shim in front of `clang` counted 1,461 calls in one full run, using 463 s of
CPU at load 30–80:

| Call | Count | CPU |
|---|---:|---:|
| `clang --version` | 782 | 182 s |
| links and compiles | 679 | 281 s |

`ld64.lld --version` calls `ld64.lld` directly, so the shim missed it, and it
costs about as much again. Two things make each call expensive:

- **The probes.** `link::identity_of` hashes the banners of
  `clang --version` and `ld64.lld --version` into the `link` key. It memoizes
  them per process, but almost every `buri` process the suite starts links
  once, so it probes every time. That's about 100 ms of CPU per process, and
  0.23 s each at load.
- **Starting the tools.** In a small `buri test`, `buri` itself uses 44 ms of
  CPU, `ld64.lld` 84 ms, the `clang` driver 72 ms and Nix's `cc-wrapper`
  bash script 44 ms. About half of `ld64.lld` and 90% of the driver run
  before `main`, in libLLVM's static initializers registering every
  `cl::opt`, AMDGPU's included. Of the rest of `ld64.lld`, a third goes to
  parsing the SDK's `.tbd` stubs for `libSystem`'s re-exports. In
  `repositories::snapshots`, those three tools are 65% of the test's CPU.

None of this is fixed yet. The options, cheapest first:

- Keep the identity probe's answer on disk, keyed by each program's resolved
  path, size and modification time, and probe only when one moves.
- Call `ld64.lld` directly with the arguments the driver adds, skipping
  `clang` and the `cc-wrapper`. That removes two of the three process starts
  per link.
- Give the devShell an `lld` linked against a static, trimmed LLVM with only
  the AArch64 and x86 targets, so the initializers have less to register.

**What else was found and left.** `macOS` holds every new executable file
for a `syspolicyd` check: about 0.2 s, longer under load, and one at a time
across the machine. A hard link or a rename skips it, but a copy or an APFS
clone pays it again. `link::place_from` already accounts for this, so only a
first build in a fresh scratch repository pays. The hang cap's `launched`
waits a flat 10 ms before its first look. Backing it off from 200 µs made no
difference to the `failing` suite, so it stayed as it was. After the stencil
change, the next per-process costs in a small `buri test` are the standard
library snapshot (16%), building the stencil key index (7%) and spawning.

**How the numbers were taken.** `samply` isn't installed, so
`nix shell nixpkgs#samply -c samply record --save-only
--unstable-presymbolicate` records a command and every child it starts,
without Developer Tools access. Weighting each sample by its
`threadCPUDelta` turns the samples into CPU per process.

### 6.22 Front-end data structures, 2026-10-05

A profile of `mixed-100k` put 30% of the active samples in checking inside
`malloc` and `free`, and spread the rest thin. So this round went after
allocations, one site at a time, ranked by a sampling allocator.

| Change | Commit | `mixed-100k` sema instructions | sema allocations |
|---|---|---:|---:|
| `main` at `a0dfb178` | | 639.3 M | 947,084 |
| Inference keeps its substitution and scope lists on the checker between bodies | `f74252fb` | 612.9 M (−4.1%) | 874,947 |
| A body's parameters are read in place, each name copied once | `26183ca0` | 599.2 M (−1.8%) | 846,002 |
| Payload patterns read the declared fields in place instead of copying the variant | `b092b603` | 589.9 M (−1.4%) | 820,178 |
| Call arguments go straight into the list holding the receiver; blocks size their statements | `3312883c` | 575.1 M (−2.4%) | 794,559 |
| Exhaustiveness column types are a `Vec<Ty>`, not a `Vec<Cow<Ty>>` built from a second `Vec` | `225662e7` | 572.0 M (−0.8%) | 779,366 |
| An exhaustiveness matrix keeps its rows end to end in one list | `325a96b8` | 565.0 M (−1.3%) | 748,389 |
| `check_derives` and `check_entry_point` copy a name only to report it | `344456d7` | 548.9 M (−2.8%) | 715,615 |
| A call's parameter types borrow a spare list from the body | `40046bcd` | 543.7 M (−0.9%) | 692,812 |
| `SourceFile::new` finds line starts with `memchr` | `15da63c5` | 534.9 M (−1.6%) | |
| Pending bounds, literal checks and pattern names reuse the body's lists | `f50b5d29` | 516.5 M (−3.2%) | 645,484 |
| `FnInfo::generics` is an `Arc<[GenericInfo]>`, completed in place | `75d4dd24` | 502.7 M (−2.8%) | 604,472 |

Instructions are a `sema` child minus a `lex+parse` child, each running its
phase ten times in one process and divided by ten, so the corpus generator and
the one-off standard library snapshot drop out. The minimum of two alternating
runs per side; load ran 25–110. Allocations are one repetition through the
counting allocator, loading included. Each row is measured against the row
above it.

Most rows are the same move: a list that lived for one body or one call now
lives on the checker and gets cleared instead of dropped.

```rust
pub(crate) struct Scratch<'b> {        // inference.rs, on Checker
    subst: Subst,
    scopes: Vec<(u64, LocalId)>,
    ty_lists: Vec<Vec<Ty>>,            // one per level of call nesting
    lit_checks: Vec<LitCheck<'b>>,
    ..
}
impl Drop for Infer<'_, '_> { .. }     // clears each list and hands it back
```

The matrix change is struct-of-arrays in miniature. Every row of a matrix has
the same width, so a row is a slice of one `Vec<&Pat>` and specializing a
matrix is one allocation instead of one per row.

`check_derives` was the surprise. It listed every component of every derived
type, with a `format!` per enum field for a name only a diagnostic reads, four
times for a type deriving four traits.

One change landed in `syntax`: a file's string literals keep their cooked text
end to end in one `String`, in the lexer and in the tree, instead of a `String`
each (`8feca4d4`). Lex+parse allocations fell 10.6% on `mixed-100k` and 28% on
`string-heavy-100k`. Instructions moved only where strings are dense:
`string-heavy-100k` lex+parse fell 2.1–2.7%, `mixed-100k` stayed within noise.

| End to end, `main` against all of it | before | after |
|---|---:|---:|
| `mixed-100k` sema instructions, per repetition | 637.6 M | 503.2 M (−21%) |
| `match-heavy-100k` | 695.3 M | 571.7 M (−18%) |
| `generic-blowup-100k` | 653.9 M | 516.4 M (−21%) |
| `string-heavy-100k` | 627.7 M | 498.9 M (−21%) |
| `saved:mixed-10k` | 89.4 M | 72.1 M (−19%) |
| `mixed-100k` sema allocations | 947,084 | 604,472 (−36%) |
| `string-heavy-100k` lex+parse allocations | 51,872 | 37,318 (−28%) |
| `mixed-100k` peak RSS after `sema` | 109.7 MB | 100.2 MB (−8.7%) |
| `generic-blowup-100k` peak RSS after `sema` | 121.9 MB | 109.5 MB (−10%) |
| cold `buri lint //...` on the monorepo copy, instructions | 19.0–19.2 G | 17.9 G (−6%) |

The lint's output is byte-identical, and the full suite passes unmodified.

**Reverted.** Making `FnInfo::params` and `generics` `Arc<[_]>` while still
building each list as a `Vec` first: the conversion's copy cost what the clone
in `check_fn` had, so instructions moved 0.3% and allocations didn't. The
version that landed builds the shells straight into the `Arc` and fills in the
bounds in place.

**How the ranking was taken.** A throwaway patch to the bench's counting
allocator recorded a backtrace for every seventh allocation inside one phase,
bucketed by the first frame in `crates/`, from a build with
`CARGO_PROFILE_BENCH_DEBUG=line-tables-only`. Type sizes came from
`RUSTC_BOOTSTRAP=1 cargo rustc -p buri-semantics --release -- -Zprint-type-sizes`.

**What's left.** By allocation count, in `mixed-100k` sema:

- `typed::Local::name` is a `String` per local, about 6%. Interning names
  needs `middle` and the backends, which read it.
- The typed tree's own lists: `vec![l, r]` per binary operator, a call's
  arguments, its type arguments.
- `resolve_scopes` copies each module's own names into `names` and then every
  prelude name into it, about 3%. `names` is public and the language server
  iterates it, so a layered lookup changes its readers too.
- `declare` keys `own` and `exports` with a `String` each.
- `typed::Expr` is 112 bytes with 16-byte alignment because
  `ExprKind::Int` holds an `i128`. Storing it as two `u64`s takes `Expr` to
  104 bytes and shrinks `Stmt` (208) and `Arm` (320) with it, but `middle`
  matches on it.

### 6.23 `buri clean` and tool processes, 2026-10-05

`buri clean` opened a full session, and opening one runs every generator in
the repository. So it built the tools, ran them, then deleted what they wrote.
In a fresh repository it even printed `dropped .buri/out` where there was
nothing to clean. Now it finds the root and deletes:

```rust
let root = root_of_cwd()?;   // was open_or_exit(&flags), which ran prepare()
```

Generators cost more than the compile because of process starts. A tool got
one bun process per request, and a cold build of `database_server` makes 31
requests of two tools:

- 16 checks of the proto inputs, plus a second round for the 13 that import
  another file. The tool answers the first with `needs`, and the build asks
  again with the files.
- One proto `generate` over all 16, and one `//tool/app_manifest` `generate`.

Loading the 316 KB proto tool costs about 0.24 G instructions before it
reads a byte, and a cold JIT makes each check cost 0.33–0.8 G. Replayed
through one warm process, all 29 checks take 2.2 G in total.

So `core/tool`'s `serve` now answers requests until its input ends. One
request and then EOF behaves as it always did. While `prepare_rules` runs,
`run_artifact` keeps each process it starts and sends the next request for
that tool to an idle one. If a kept process gives anything short of an
answer, the request runs again in a process of its own. A failing tool runs
twice, and its diagnostics are the ones it always gave. Generators written by
users don't change: the toolchain writes the `main` that calls `serve`.

Cold, on the monorepo copy, one run per side at load 10–30, alternated where
under 3%:

| | `main` | now |
|---|---:|---:|
| `buri clean`, fresh: own instructions | 3.46 G | 0.02 G |
| `buri clean`, fresh: bun runs, instructions | 37, 30.5 G | 0 |
| `buri clean`, fresh: wall | 1.5 s | 0.03 s |
| `build //apps/database_server`: bun runs | 31 | 11 |
| `build //apps/database_server`: bun instructions | 17.6–18.1 G | 11.3–11.8 G (−36%) |
| `build //apps/database_server`: bun user+sys | 2.1–2.6 s | 1.4–1.5 s |
| `query`, which runs every generator: bun runs | 37 | 17 |
| `query`: bun instructions | 31.3–31.5 G | 24.1–25.5 G (−21%) |

Wall time moved within noise. The checks still fan out across cores, so
`database_server` still starts about ten processes.

**What's left.** The proto `generate` over 16 schemas is 3.4 G even warm.
That's `std/proto`'s own work, not process overhead. Capping processes per
tool would push the checks toward 2.2 G, but they'd run in series, which costs
wall time.

Running a tool by hand with a regular file as standard input now hangs at the
end of the input. That's bun: paused `process.stdin` on a file never emits
`end`. A pipe, which is what the build uses, ends normally.

**A hang, fixed.** A cold `buri test` once sat in its generators for 21
minutes. macOS has no `pipe2`, so a tool started while another thread made a
pipe could inherit both ends. A kept tool holding another's input kept it from
ever reading the end, and `KeptProcess::finish` waited forever. One process
per request never noticed: it exited after one line. Tools now start through
`spawn::start_alone`, which holds `FORKS` alone while every other start holds
it shared:

| Cold runs, before → after | hung | tools started holding another's pipe |
|---|---:|---:|
| monorepo copy, `test` and `build //...` | 5 of 150 → 0 of 150 | |
| 64 rules of one tool, `build //...` | 1 of 5 → 0 of 60 | 14 of 64 in one run → 0 of 3,840 |

Wall time didn't move. Alternating on the monorepo copy at load 22–33, a cold
`build //...` took 2.21 s before and 2.25 s after (eight runs each). A cold
`test //...` took 14.2 s and 13.8 s (four each).

### 6.24 The linker is asked once per toolchain, 2026-10-05

§6.21 found every `buri` process asking `cc --version` and
`ld64.lld --version` for the `link` key, and every fresh repository asking
the driver for its `-###` line. Both answers now live in `BURI_HOME`
(`~/.buri`), so they outlast the process:

```text
~/.buri/linker-identity/<key>   the hash of the two banners
~/.buri/link-replay/<key>       the linker command the driver printed
```

The identity's key is what could change a banner: each program's path, the
file it resolves to, its device, inode, size, mtime and ctime, the variables
`shapes_the_link` names (`PATH`, `NIX_*`, `SDKROOT`, …), and on macOS the
`xcode-select` link. A program written anew re-probes. A program that didn't
answer isn't remembered. The replay key gains the running `buri`'s identity,
since the store now outlives one toolchain build. A link with the cache off
keeps nothing, as before.

Two end-to-end tests in `build/hermeticity.rs` drive a counting `cc` on
`PATH`. A second process doesn't ask the version, and rewriting the fake, with
the same bytes or different ones, asks again. A second repository links
without starting the driver.

**A small `buri test` in a fresh repository** (`testing/caching`), three
alternating runs each at load 16–18:

| | before | after |
|---|---:|---:|
| CPU, `buri` and every child | 0.24 s | 0.08 s |
| wall | 0.39 s | 0.24 s |
| `buri`'s own instructions | 217 M | 195 M |

**In the whole suite**, counted by setting `CC` to a logging shim, one run each:

| `cc` call | before | after |
|---|---:|---:|
| `--version`, from `buri` and harnesses | 238 | 30 |
| `-###` | 23 | 24 |
| native test harness links (`-o …/program`) | 423 | 423 |
| everything | 753 | 546 |

`ld64.lld --version` comes from the same probe, so it fell by the same 208.
The 30 left are most likely fakes with their own paths, homes of their own,
and processes that started before the first answer was written. This shim
replaces `CC`; §6.21's sat under the cc-wrapper, in front of
`clang`, which is why its counts are higher. The `-###` count didn't move:
the suite's repositories already found their replay in the repository cache.
The per-user replay store helps a fresh clone or a `buri clean`, not the suite.

**The full suite didn't measurably move.** The archives of both sides,
alternating, at `--test-threads 3`:

| Run | Load | Wall | User | Sys |
|---|---|---:|---:|---:|
| before | 21–46 | 490 s | 1,082 s | 408 s |
| after | 21–56 | 592 s | 1,079 s | 416 s |
| before | 56–69 | 591 s | 993 s | 430 s |
| after | 37–64 | 485 s | 947 s | 413 s |

The small test suggests about 0.15 s of CPU per removed probe, so about 30 s of
the suite's 1,400 s. Other agents shared the machine and runs of one build
swung by 100 s, so that's below what these runs can show.

**What's left, and why.**

- **The native harness links**, 423 per run, are 78% of the driver calls
  left. `tests/native/stencil.rs` links its programs through `product_cc()`,
  so each pays for the cc-wrapper's bash, `clang` and `ld64.lld`. Routing them
  through `CDriver::link` would replay. It isn't behaviour-identical, though:
  the product stages objects in a link directory, names them relatively and
  links the runtime archive only when an object names one of its symbols,
  while the harness always passes it. Changing the harness to the product's
  link is a separate decision.
- **The cross `--target` probe** (`accepts_target`) still runs once per
  process that cross-links. Few tests do.
- **A trimmed `lld`.** About half of `ld64.lld` is libLLVM's static
  initializers. An `lld` linked against a static LLVM with only the AArch64
  and x86 targets would start faster on every link. It needs a custom nix
  derivation and a rebuild of LLVM, so it's a devShell change, not a code one.
- **A store for `buri-stencil`'s build script**, 12–21 s on a cold build.
  The runtime archive's store (`cli/build.rs`, `SharedArchive`) works because
  its stamp hashes the whole environment, minus a list argued to reach nothing,
  and the input tree. The stencil key would need the same: the generator
  sources, `buri-hash`, `cc` and its banner, `TARGET`, and every variable the
  cc-wrapper turns into flags. That means moving the stamp code into something
  both build scripts share, or copying 300 lines. A stale key here ships wrong
  machine code, and the win is cold builds only, so it's left for now.

### 6.25 Data structures in the back half, 2026-10-05

Even on one thread, `malloc` and `free` were 30% of the CPU in a native
`lower` row on `mixed/100k`, and 27% in a JavaScript one. The allocation
profile was flat: no site above 7%. So the work was a dozen small changes to
`middle`, `stencil` and `js`, each picked off a profile and each one
output-identical.

Instructions are a `lower` child minus a `sema` child under `/usr/bin/time
-l`, best of two alternating runs. Allocations are `mixed/10k`, counted.

| Change | Commit | `mixed/100k` instructions | `mixed/10k` allocations |
|---|---|---:|---:|
| `regalloc` keeps per-block uses in one table stamped with the block, not a `HashMap<u32, Vec<usize>>` per block | `a1b560eda` | native −4.8% | native emit −10.7% |
| `Jit::layout_of` and friends return the `Rc<Layout>` `Layouts::shared` already holds, not a clone | `a4a0fecc0` | native −2.7% | native emit −8.8% |
| `rc::preorder`, `subtree_sizes` and `fresh` walk children and tails through a closure, not a collected `Vec` | `331601330` | js −3.4%, native −4.3% | native prepare −15%, js −7.3% |
| `Jit::promote` returns before building its tables when no back edge exists; `assemble_unit` borrows relocation names | `2dd8bf398` | native −3.4% | native emit −10% |
| Call sites borrow `FrameSig`s; `pin_call_values` uses the stamped table | `d371529d4` | native −2.3% | native emit −5.9% |
| `clean_body` resolves its map without cloning it; local and intrinsic names are one buffer each; local names are a `Vec` by `LocalId` | `020105b82` | js −5.5% | js −10.4% |
| A monomorphized symbol is written once; `strongly_connected` returns one flat member list (`Components`) | `1bc5ed863` | js −3.9%, native −3.4% | mono −23%, js prepare −21% |

The table each block used to rebuild is one entry per value, reused across
blocks:

```rust
struct BlockUses { block: u32, count: u32, first: usize, last: usize }
// An entry whose `block` isn't the current one reads as "no use here".
```

| End to end, before → after | `mixed/10k` | `mixed/100k` |
|---|---:|---:|
| `lower+js`, net of `sema` | 415 M → 368 M (−11%) | 3,459 M → 3,054 M (−12%) |
| `lower+macos-arm64` | 561 M → 445 M (−21%) | 5,130 M → 4,106 M (−20%) |
| `lower+linux-x86_64` | 590 M → 481 M (−18%) | 5,492 M → 4,485 M (−18%) |
| peak RSS, `lower+js` | 36 → 37 MB | 219–223 → 224–227 MB |
| peak RSS, `lower+macos-arm64` | 47 → 46–47 MB | 274 → 273–276 MB |

| `mixed/10k` allocations | before | after |
|---|---:|---:|
| monomorphize | 56,412 | 43,434 (−23%) |
| JavaScript `prepare` | 35,090 | 27,716 (−21%) |
| JavaScript `prepare` + emit | 327,244 | 262,501 (−20%) |
| native `prepare` | 128,715 | 99,137 (−23%) |
| native emit | 477,393 | 304,290 (−36%) |

| Cold `buri test //...` on the monorepo copy, two alternating pairs | before | after |
|---|---:|---:|
| instructions, `buri` process | 77.3–77.7 G | 68.5–68.6 G (−12%) |
| peak memory | 1,420–1,519 MB | 1,382–1,430 MB |
| wall, at load 13–23 | 14–15 s | 14–15 s |

Peak memory barely moved: the allocations removed were short-lived. Every
object and JavaScript bundle of `mixed/10k`, `mixed/100k` and four saved
corpora, on every target, is byte-identical by SHA-256: 1,242 outputs. So
are the monorepo run's 889 objects.

**Tried and dropped.** The stencil emitter sizes two tables to the whole
program for every part (`Jit::plan`), and one of them, `dirty`, is written
and never read. Dropping both left instructions where they were. The CPU
profile charged it a 6% `memset`, which retires few instructions.

**Type sizes.** `RUSTC_BOOTSTRAP=1 cargo rustc -p buri-middle --lib --
-Zprint-type-sizes` works with the devShell's stable compiler:

| Type | Size | Largest variant |
|---|---:|---|
| `ir::Inst` | 80 B, align 16 | `CallIntrinsic`, 72 B: three `Vec`s and a `String` |
| `ir::Term` | 72 B | `Branch`, `Switch`, 68 B |
| `ir::Block` | 120 B | |
| `layout::Layout` | 72 B | |

Most instructions need 24 bytes or less. `Const`'s `i128` sets the
alignment, and a call's `dests`, almost always one value, is a `Vec`.
Shrinking `Inst` touches every match on it in four crates, so it's left as
the next step rather than done here.

**What's left.** `Scan::children` in `rc` still collects children and modes
into `Vec`s and clones the live set per node. The JavaScript AST clones an
`Ident(String)` every time it copies an expression, 7% of that backend's
allocations. Monomorphization and `inline` copy whole typed bodies, which is
most of what `prepare` allocates now.

**How the numbers were taken.** The samples came from a throwaway build,
none of it committed:

- the `alloc-counter` allocator, extended to count the lowering phases and to
  keep the backtrace of every 101st allocation, or of every 64 KiB, grouped
  by the innermost frame in this repository. Build it with
  `--profile validate` and line tables: fat LTO folds inlined frames into
  the wrong lines;
- a `BURI_ONE_THREAD` check in `parallel::width`, so `samply` sees one
  thread and CPU tracks instructions instead of `malloc`'s lock contention;
- `--rss-child` phases that stop after monomorphization, `prepare` and
  `lower::run`, to split a row's instructions.

Two traps: a profile of the counting build charges the atomic increments to
`finish_grow`, and `git diff` here runs difftastic, so save a patch with
`--no-ext-diff`.

### 6.26 Harness links replay the product's linker line, 2026-10-05

§6.24 left 423 native harness links per suite run, each going through the
driver. They actually cost more than it said. The harness never passed
`-fuse-ld=lld`, so each link started four processes: the cc-wrapper's bash,
`clang`, cctools' `ld` wrapper (bash again) and Apple's `ld64`. The product
links with `ld64.lld`.

**Which links are about the link.** Per run, from the `CC` shim's log:

| Call site | Links | Asks about |
|---|---:|---|
| `stencil.rs`: `build_with`, `build_tests_with`, corpus `link_and_run` | 211 | the program |
| `conformance.rs` `linked` | 109 | the program |
| `agreement.rs` | 70 | the program |
| `fuzz.rs` native search | 34 | the program |
| `llvm.rs` `build_at`, `build_tests_as` | 0 by default | the program |
| `stencil.rs` thread-door test | 1 | what `-dead_strip` kept, through `nm` |
| `runtime.rs`, `float_parity.rs` C drivers | 2 | the program, but they compile C too |

These tests are about the link and were already on other paths:

- the archive-size ceilings, which go through `link::run`;
- the cross ELF tests, which call `ld.lld` directly;
- `link.rs`, which calls `CDriver::link`;
- everything that runs the `buri` binary.

The 63 `-c` calls are probe and stub compiles. They need the driver.

**What changed.** The 424 "program" links now call one helper:

```rust
// cli/tests/native/shared.rs
pub fn link_program(objects: &[PathBuf], binary: &Path) -> Output {
    buri::build::link::product_link(&staged().0, objects, binary)...
}
```

`link::product_link` uses the same `Replay`, under the same key, as
`CDriver::link` when runtime linking is on. It keeps the command in
`~/.buri/link-replay/` even though the harness has no cache, because almost
every test is its own process. It swaps `-o artifact` for the binary's path,
because one test process links in parallel threads. The harness hard-links the
runtime archive into its staging directory once, as `libburi_rt.a`, so the
command line names it exactly the way the product's does. As before, the
archive is always linked. If there's no line to replay, or replaying it fails,
the driver runs instead. The thread-door test and the C drivers keep the
driver (`shared::driver_link`, `product_cc`).

**Same behaviour.** Both sides pass all 2,253 tests. Every corpus and
conformance program the base run linked, 321 of them, was linked both ways,
run under the heap check, and compared on status, stdout and stderr. 320
matched. `server-tls` prints the port the OS gave it, so two runs of the same
binary don't match either.

**In the whole suite**, with `CC` set to a C shim that logs each call's
children's CPU, from the archives of both sides, alternating, at
`--test-threads 3`:

| `cc` call | before | after |
|---|---:|---:|
| harness program links | 425 | 1 |
| `-c` compiles | 63 | 63 |
| `--version`, `-###`, product links, C drivers | 34–62 | 34–39 |
| everything | 522–550 | 98–103 |
| CPU in `cc` and its children | 237–239 s | 22–26 s |

The bash count falls by 848: two per removed link. `ld64.lld` now starts 424
times where `ld64` used to, so the number of linker processes doesn't change.
Replaying one corpus program's link takes 71 ms of CPU, against 183 ms through
the driver, at load 37.

| Run | Load | Wall | User | Sys | User + sys |
|---|---|---:|---:|---:|---:|
| before | 15–26 | 457 s | 1,028 s | 387 s | 1,415 s |
| after | 14–45 | 415 s | 809 s | 339 s | 1,148 s |
| before | 6–21 | 430 s | 934 s | 390 s | 1,324 s |
| after | 17–23 | 367 s | 798 s | 330 s | 1,128 s |

That's 195–267 s less CPU per run, or 15–19%, in line with the 213–215 s
the shim stopped seeing. The run is still over §6.21's five minutes of wall
time.

### 6.27 Smaller IR and typed-tree nodes, 2026-10-05

Three follow-ups from §6.22 and §6.25: two `u128` fields that 16-byte aligned
whole nodes, and a `String` per local.

```rust
pub struct Magnitude([u64; 2]);   // typed.rs: an integer literal, 8-byte aligned
pub struct Name(&'static str);    // name.rs: interned, Copy

Inst::Call { dest: ValueId, func, args }               // was dests: Vec<ValueId>
Inst::CallIntrinsic { dest, key: Box<str>, args: Box<[ValueId]> }
```

| Type | Before | After |
|---|---:|---:|
| `ir::Inst` | 80 B, align 16 | 40 B, align 8 |
| `ir::Const` | 32 B, align 16 | 24 B |
| `typed::Expr` | 112 B, align 16 | 104 B, align 8 |
| `typed::Pattern` | 80 B, align 16 | 64 B |
| `typed::Stmt` | 208 B | 184 B |
| `typed::Arm` | 320 B | 288 B |
| `typed::Local` | 48 B | 40 B |

`ir::Term` (72 B) and `ir::Block` (120 B) didn't move: neither holds an
`Inst` or a `Const` inline.

**`Inst`, `527f5a88`.** `Magnitude` alone left `Inst` at 80 bytes, because
`CallIntrinsic` held three `Vec`s and a `String`. Every call lowering makes
`vec![dest]`, so a call's results became one `dest`. That's 56 bytes. Boxing
the intrinsic's key and arguments, and a string constant's text, gets the
rest of the way to 40. Each step was measured: 56 bytes took native lowering
down 0.9%, 40 took it down another 1.0%.

**`Expr`, `49d11d06`.** `ExprKind::Int` and `PatKind::Int` hold a
`Magnitude`. `matches!(k, ExprKind::Int(2, false))` in tests becomes
`ExprKind::Int(m, false) if m.get() == 2`.

**`Local::name`, `fae72e66`.** There was no string interner, so `name.rs`
adds one shaped like the type table: 64 shards, entries leaked, bounded by
distinct spellings. Monomorphization and inlining copy bodies, and each copy
used to clone every local's name.

Instructions are a phase child net of the one before it (`sema` minus
`lex+parse`, lowering minus `sema`), minimum of two or three alternating runs.
Each row is against the row above.

| `mixed/100k` | `main` + §6.25 | `Inst` | `Expr` | `Name` |
|---|---:|---:|---:|---:|
| `sema` | 566.8 M | 563.1 M | 560.5 M | 550.3 M (−1.8%) |
| `lower+js` | 3,056.8 M | 3,031.8 M | 2,994.3 M (−1.2%) | 2,981.0 M |
| `lower+macos-arm64` | 4,140.5 M | 4,058.8 M (−2.0%) | 3,983.8 M (−1.8%) | 3,975.7 M |
| `lower+linux-x86_64` | 4,504.7 M | 4,403.4 M (−2.2%) | 4,350.4 M (−1.2%) | 4,351.5 M |
| peak RSS, `lower+macos-arm64` | 261.5 MB | 245.9 MB | 246.5 MB | 238.2 MB |

Deltas under 0.7% are left out: `lower+js` doesn't touch `Inst`, and it
still read −0.7% on the `Inst` row, so that's the floor here.

| Allocations, one counted run | before | after |
|---|---:|---:|
| `mixed/100k` `sema` | 524,029 | 486,111 (−7.2%) |
| `mixed/100k` monomorphize | 405,565 | 367,210 (−9.5%) |
| `mixed/100k` monomorphize + JavaScript | 2,966,180 | 2,882,560 (−2.8%) |
| `mixed/100k` monomorphize + native | 4,326,340 | 4,153,637 (−4.0%) |

Only `Name` moved the front end and monomorphization. `Inst` took 23,600
off native: one `dests` list per call.

| Cold `buri test //...` on the monorepo copy | before | after |
|---|---:|---:|
| instructions, `buri` process | 68.7–69.1 G | 67.7 G (−1.5%) |
| peak memory | 1,483–1,486 MB | 1,317–1,386 MB |
| allocations | 53.1 M | 51.0 M (−3.8%) |
| `middle` phase allocations | 7.37 M | 6.37 M (−14%) |

Every JavaScript bundle and object of `mixed/10k`, `mixed/100k` and six saved
corpora, on `js`, `macos-arm64`, `linux-x86_64` and `linux-arm64`, is
byte-identical by SHA-256 after each commit: 1,370 outputs. The stencil
backend has no `macos-x86_64`.

**Not done.** The typed tree's small lists and `resolve_scopes`:

- `vec![l, r]` is already one exact allocation, so a boxed slice saves 8
  bytes per list and no allocations. `ExprKind` is 80 bytes because
  `CallTrait` and `Intrinsic` each hold two or three lists, so shrinking it
  means converting every list, and the middle passes push to them.
- `SmallVec<[Expr; 2]>` inline would make `ExprKind` over 200 bytes.
- A layered `resolve_scopes` changes the language server, which iterates
  `names`.

**How the numbers were taken.** The same throwaway bench patch as §6.25,
plus two switches: `BURI_DUMP=<dir>` writes a `--rss-child` lowering's
output for hashing, and `BURI_ALL_TARGETS=1` makes the child emit the
`--targets=` it was given even above the size where it would only take the
host. One cold monorepo run hung for 21 minutes on its `bun` generators,
with 2.8 G instructions retired; it was killed and rerun, and the hang didn't
recur in the runs after it.

### 6.28 Snippets already share the standard library, 2026-10-05

`cli/tests/README.md` said each of `recovery`'s 5,200 analyses type-checks the
whole standard library, at about 650 CPU-seconds. That was written on 10-02,
the day before §6.15. Since then a snippet resumes from the process's
snapshot, and `load_all_std` on a seeded loader finds every module already in
`by_path`:

```rust
// driver::analyze_snippet_on
analyze_on(ws, map, cache, Opening::Library, Scope::All, |loader| { … loader.load_all_std(); … })
// analyze_on
let snapshot = snapshot::of(opening, matches!(scope, Scope::All));  // one per process
```

So the per-snippet cost the lead described is gone. What's left is one
snapshot per **process**, and nextest starts a process per test. Here's what a
snapshot costs to build, single-threaded, in the test profile:

| Snapshot | Instructions |
|---|---:|
| `Builtin`, no bodies | 30.6 M |
| `Builtin`, with bodies | 33.5 M |
| `Library`, no bodies | 150.0 M |
| `Library`, with bodies, which every snippet uses | 276.5 M |
| one trivial snippet on top of a built `Library` | 0.43 M |

Every test the binaries below select, each in its own process as nextest runs
it, four at a time. A throwaway patch logged each snapshot build's thread
instructions and CPU. The CPU column is `user + sys` of a nextest run of the
same filter, cargo included:

| Binary | Tests | Instructions | Snapshots built | Snapshot share | CPU |
|---|---:|---:|---:|---:|---:|
| `recovery` | 38 | 53.0 G | 5 | 2.9% | 12.0 s |
| `checking` | 29 | 8.0 G | 3 | 11.4% | 4.5 s |
| `fuzz` | 41 | 85.8 G | 14 | 3.7% | 28.4 s |
| `language` | 103 | 79.7 G | 9 | 1.0% | 256.6 s |
| `buri` unit tests | 371 | 52.5 G | 114 | 63.6% | 44.2 s |
| `native`, `agreement::` | 71 | 35.9 G | 70 | 58.6% | 35.6 s |
| `docs` | 56 | 40.9 G | 18 | 9.9% | |

`language`'s instructions leave out the `buri` processes it starts, which is
where its CPU goes. `recovery::a_syntax_error_does_not_become_a_type_error`
takes 26.8 G instructions and 3.3 s of CPU for its 7,000 analyses, not 650 s.

**Across the full suite it's 2%.** The same patch, over one full run at the
default thread count:

| Who builds it | Builds | Instructions | CPU |
|---|---:|---:|---:|
| `buri` processes, `Builtin` | 2,316 | 82.5 G | 9.6 s |
| `buri` processes, `Library` | 104 | 31.5 G | 3.6 s |
| test processes | 397 | 111.8 G | 13.0 s |
| **total** | | **225.7 G** | **26.2 s** |

That run took 274 s of wall time and 1,200 s of CPU, at load 35–51. One
without the patch took 277 s and 1,202 s, at load 26–81. Both had the tests
built already.

**Nothing changed.** Within a process the snapshot is already shared. Sharing
it across processes means writing the checked library to disk and reading it
back, a serializer for every checker table, to save at most 26 s of CPU: about
2.6 s of wall time on ten cores. The unit tests and `agreement` are the only
binaries where it's most of the cost, and together they spend 80 s of CPU.
The suite's time is in the places §6.21 names: the `buri` processes the tests
start, and the linker tools.

### 6.29 Wide payloads are words, not one integer, 2026-10-05

A `--release` build of a program with a 200-field struct (`Big`, 4,800 bytes)
took a minute or two when the struct sat in a `Result` or was matched three
levels deep. `opt -time-passes` on the module the backend emitted:

```text
InstCombinePass   56.9 s of 58.0 s   690 G instructions
```

The trigger is a tagged enum's payload area. `repr.rs` held it as one integer
of exactly its bytes, so `Result<Option<Big>, Str>`'s payload was an `i38400`,
built and taken apart like this, once per word:

```llvm
%pay.w  = zext i64 %f17 to i38400
%pay.sh = shl i38400 %pay.w, 1088
%pay    = or i38400 %pay.prev, %pay.sh        ; 600 of these, chained
...
%pay.sh2  = lshr i38400 %pay, 1088
%pay.cut  = trunc i38400 %pay.sh2 to i64     ; and 1,200 of these
```

Every one of those is a 600-word `APInt` operation, and InstCombine revisits
the chain for each `lshr`. The struct itself was never the problem: its 600
slots travel as an ordinary literal struct, and `insertvalue`/`extractvalue`
on that is cheap.

**The fix:** a payload area wider than 64 bytes is an array of the widest of
`i64`, `i32`, `i16` and `i8` that divides it (`repr::blob_elements`). A field
goes in and out a word at a time, with sub-word shifts only where a field
doesn't fill one, so nothing wider than 64 bits reaches InstCombine. Payloads
of 64 bytes or less keep the integer form they had.

`buri build --release`, emit phase, `BURI_PROFILE=1`:

| Program | Before | After |
|---|---:|---:|
| `Result<Option<Big>, Str>`, built and matched | 721.5 G, 59.8 s CPU | 16.9 G, 1.4 s |
| `Option<Holder>` matched three levels deep in one pattern | 947.6 G, 75.0 s | 23.6 G, 1.8 s |
| `e2e.rs`'s `deep_big`, still matching one level at a time | 360.3 G, 32.3 s | 301.3 G, 27.9 s |

`deep_big` now takes the deep pattern and the `Result<Option<Big>, Str>` too.
Its remaining cost isn't payloads. Its derived `Equal`, `Hash` and
`Show` on `Big` emit 246k, 122k and 138k lines of IR before optimization.

**Nothing else moved.** Over `--set=native`, every one of the 195 distinct
units the LLVM rows emit hashed the same before and after: none has a payload
wider than 64 bytes. So the optimize time and the generated code on the bench
corpora are unchanged by construction. `native::llvm`'s
`a_wide_payload_is_held_as_words_rather_than_one_wide_integer` bounds the
widest integer in such a program's IR at 512 bits.

### 6.30 Derived functions are linear in the fields, 2026-10-05

§6.29's `Big` (200 `Str` fields, 600 slots) with `derive Equal, Hash, Show`.
The derive tree was already linear: `a.f0 == b.f0 && …`, one hole per field.
The blow-up was below it, in two places.

**Every field read took the whole struct apart.** The LLVM backend's
`get_field` disassembled the aggregate, an `extractvalue` per slot, then used
the field's three:

```llvm
%slot    = extractvalue { ptr, ptr, i64, … } %x, 0
...                                              ; 600 of these per field read
%slot599 = extractvalue { ptr, ptr, i64, … } %x, 599
```

`Equal` reads 400 fields, so 241k of its 245k instructions were these. They
were dead and cheap, though: InstCombine drops them early, and fixing this
alone moved the release build by 2%.

**`Show` joined 401 parts by a chain of concatenations.** The derive hands
them to a shared joiner, `derive$join$401`, and `lower` turned that template
into 400 `str.concat`s, each open-coded at about a hundred instructions. That
was 42k instructions in one function with 1,203 parameters live across it:
`ConstraintElimination` and the greedy register allocator spent most of a
minute's work on it.

**The fixes:**

- `get_field` extracts only the field's slots (`repr::disassemble_range`), and
  so does a niche tag test.
- A template of more than 16 parts (`lower::CONCAT_CHAIN_MAX`) puts its parts
  in a `[Str]` and calls `list.join` with an empty separator, once.
- A derived `Show` with more than 16 parts writes that template inline. A
  joiner of that arity cost more than the list: every part a parameter, each
  given a count going in and dropped by the caller coming out.

LLVM instructions in the derived functions, debug-profile IR:

| Function | Before | After |
|---|---:|---:|
| `Equal` | 245,396 | 6,596 |
| `Hash` | 122,004 | 2,604 |
| `Show`, with its joiner | 177,822 | 15,652 |
| per field per operation | 908 | 41 |

`buri build --release`, emit phase, `BURI_PROFILE=1`:

| Program | Before | After |
|---|---:|---:|
| `Big`, no derives | 13.6 G | 13.6 G |
| `derive Equal` | 20.0 G | 18.9 G |
| `derive Hash` | 16.4 G | 15.8 G |
| `derive Show` | 74.7 G, 7.9 s | 30.4 G, 2.3 s |
| `derive Equal, Hash, Show` | 86.7 G, 9.1 s | 40.5 G, 3.2 s |

The stencil backend reads a field as an offset load and was never affected.
Its emit phase is under 0.1 G either way.

`e2e.rs`'s `deep_big` test process went from 364.7 G and 34.6 s CPU to 308.9 G
and 25.9 s. Its derived functions are now small. What's left is `main` itself:
77k instructions of 600-slot aggregates passed through `insertvalue` and
`extractvalue`, and `llc -O2` spends 170 G on that unit.

**Nothing else moved.** Every `lower` row of `--set=native --rss` is within
1.6% of its old instruction count, which is the noise of a parallel row (§8).
`native::llvm`'s
`a_wide_structs_derived_functions_are_a_few_instructions_per_field` bounds the
three at 100 instructions per field per operation, and `native::e2e`'s
`a_long_template_reads_the_same_and_leaks_nothing` checks a long template's
text and its blocks.

### 6.31 A large value copied slot by slot is quadratic in `llc`, 2026-10-05

`deep_big`'s `--release` emit phase was 261.8 G instructions, and
`llc -O2 -time-passes` on its main unit was 174 G:

```text
Machine Instruction Scheduler          72.1 G
PostRA Machine Instruction Scheduler   53.6 G
Greedy Register Allocator              20.7 G
AArch64 Instruction Selection          15.3 G
```

After `opt -O2`, `main` is 55k instructions: 20k `store`s, 20k GEPs and 7k
`extractvalue`s. They come from moving `Big` (600 slots, 4,800 bytes) one slot
at a time: out of the 600-slot return of each `big` call, and into the
entry-block scratch buffers that runtime calls and glue take a pointer to.
Calls split scheduling regions, so each move is one region of about 1,200
memory operations, and both schedulers are quadratic in a region's size.
`-enable-misched=false -enable-post-misched=false` takes `llc` from 174 G to
53.6 G.

A synthetic unit has twelve calls returning such a value, each copied into
three buffers. `llc -O2` on it:

| Fields | First-class aggregate, a store per slot | `sret` into an `alloca`, `memcpy` |
|---:|---:|---:|
| 2 | 0.17 G | 0.16 G |
| 4 | 0.27 G | 0.19 G |
| 8 | 0.51 G | 0.29 G |
| 16 | 1.9 G | 0.16 G |
| 32 | 6.7 G | 0.16 G |
| 64 | 21.7 G | 0.16 G |
| 128 | 63.3 G | 0.16 G |
| 200 | 125.7 G | 0.16 G |

Keeping the value in SSA inside the function doesn't help. Spilling the
returned value once and `memcpy`ing it on costs the same 125.8 G, and taking
the return by `sret` and then loading its slots still costs 61.9 G. The value
has to stay in memory from the call that makes it to the copy that consumes it.

**The fix:** a value wider than 256 bytes (`repr::WIDEST_IN_REGISTERS`, from
the table's crossover) is held in memory end to end.

```llvm
; `main` after `opt -O2`: `big` writes its result into the caller's buffer,
; and the value moves into an `Option`'s payload as one copy.
call fastcc void @"main_buri$big$zxuhxr"(ptr nonnull sret([4800 x i8]) %out2, ptr null, ptr nonnull @buri.str.0, i64 -9223372036854775807, i64 3)
call void @llvm.memcpy.p0.p0.i64(ptr align 1 %pay, ptr align 16 %out2, i64 4800, i1 false)
```

- **Values:** an SSA value is the address of an entry-block `alloca` holding
  its memory form (`repr.rs`'s offsets). Values are immutable, so a copy
  shares the address, and a large field is a GEP into its owner.
- **Calls:** a large argument is its address, and a large result comes back
  through `sret` into the caller's buffer. A closure's thunk passes both on.
- **Moves:** a store into a heap block, an environment or a scratch buffer is
  one `memcpy`. A read out of a heap block is a `memcpy` into a buffer of its
  own, because the block may be freed first.
- **Block parameters:** each has a buffer of its own and no phi. Each edge
  copies its argument in, through an edge block where the terminator has other
  edges, and through temporaries where an edge has two or more copies, because
  the copies are a parallel assignment.
- **Attributes:** a field loaded out of an indirect parameter isn't argument
  memory, so `attrs::decorate` widens such a function to the default location,
  and `sret` makes it write argument memory.

`llc -O2` on `deep_big`'s main unit, after `opt -O2`, `/usr/bin/time -l`:

| | Before | After |
|---|---:|---:|
| instructions retired | 174.5 G | 17.8 G |
| both schedulers, `-time-passes` | 125.7 G | 5.3 G |
| `main`, instructions | 55,216 | 2,514 |
| the unit's `insertvalue`s and `extractvalue`s | 9,615 | 6 |
| the unit's `store`s | 22,743 | 3,337 |

What's left is per field and linear: the derived `Show`, the release and retain
glue, and `big` building its 600 slots. The greedy register allocator is the
largest pass on it, at 6.7 G.

**Nothing below the threshold moved.** All 641 release objects of 104
programs, every `cli/tests/matches` batch and every fourth `cli/tests/growth`
case, are byte-identical before and after: a value of 256 bytes or less takes
the old path at every site.

`native::llvm`'s `a_large_value_crosses_calls_by_pointer_and_moves_by_memcpy`
bounds the IR: no signature in the unit has 64 or more parameters or a result
of 64 or more members, and the functions passing `Big` around hold at most
one `insertvalue` or `extractvalue` per field. `native::e2e`'s
`a_large_struct_held_deep_inside_options_lists_and_records_leaks_nothing` runs
`deep_big` under the heap check, and
`a_tree_boxing_a_large_enum_is_built_walked_and_dropped` covers `ui/node`'s
shape, a small struct boxing a large enum, which a slot-by-slot read of the
box got wrong.

### 6.32 Large data shapes, 2026-10-06

Every shape at four sizes, built three ways under `BURI_PROFILE=1`. Each cell is
instructions at the largest size, before and after. Debug and JavaScript count
the phase that grew; `--release` counts `emit`. The "after" column includes
§6.31.

| Shape, largest size | Debug (stencil) | `--release` (LLVM) | JavaScript |
|---|---:|---:|---:|
| enum of 400 unit variants, derives | emit 17.1 G → 0.08 G | 346 G → 2.6 G | linear |
| enum of 400 payload variants, derives | panicked from 100 → 0.23 G | 325 G → 38 G | linear |
| variants carrying 200 fields | emit 1.2 G → 0.15 G | 221 G → 46 G | linear |
| `Option`/`Result` 100 deep | linear | 213 G → 30 G | linear |
| chain of 100 enums | linear | 100 G → 12 G | linear |
| tuple of 400 | emit 1.24 G → 0.15 G, check 0.09 G → 0.04 G | 398 G → 38 G | linear |
| records of 200 fields in lists and maps | emit 0.41 G → 0.13 G | 352 G → 18 G | linear |
| 400 records of 12 fields | linear | flat | linear |
| matches of 400 arms | emit 0.84 G → 0.09 G, check 0.14 G → 0.06 G | 5.5 G → 4.1 G (§6.33) | emit 1.16 G → 0.13 G |
| template of 400 holes | linear | 34.5 G → 15.4 G | linear |

Eight causes, each fixed and bound by a test:

- **A derived `compare` on an enum was `n²` arms.** Each arm for the left value
  matched the right one against every variant. Past eight variants it ranks
  both sides and compares the ranks (`derives.rs`, `RANKED_COMPARE_MIN`).
  `native::stencil`'s `a_many_variant_enums_derived_functions_are_linear_in_its_variants`.
- **A stencil retain or release wrote the whole walk in place.** A match makes
  one per arm, so a match over `n` counted variants was `n²` tests, and at 100
  variants a function passed the 1 MB a conditional branch reaches. A walk
  heavier than 16 calls the type's glue (`emit.rs`, `RC_OUT_OF_LINE`).
  `native::stencil`'s `a_match_over_many_counted_variants_is_linear_in_its_arms`.
- **A long template was one list of every part,** three stores a part with no
  call between them, and `llc`'s schedulers are quadratic in that line. It's
  joined 32 parts at a time (`lower.rs`, `JOIN_PIECE`).
  `native::llvm`'s `a_long_template_is_joined_in_pieces_of_bounded_size`.
- **The stencil backend counted a value's reads by scanning the function,**
  once per branch it might fuse. It counts them once (`jit.rs`, `Fn2::uses`).
  The debug-build IR verifier kept dominator flags per block pair; it builds the
  dominator tree instead. `build::profile`'s
  `a_debug_builds_emission_is_linear_in_a_functions_length`.
- **The JavaScript folder cloned the rest of a match chain at every arm.** It
  moves it. `build::profile`'s `javascript_emission_is_linear_in_a_matchs_arms`.
- **Reachability over a pair copied every earlier row per arm.** The arms'
  matrix keeps each constructor's specialization and extends it.
  `build::profile`'s `checking_a_match_over_pairs_is_linear_in_its_arms`.
- **A `let` walked the value's type once per name it binds.** It walks it once.
  `build::profile`'s `checking_a_pattern_is_linear_in_the_names_it_binds`.
- **`rc` copied its live set at every node and every branch.** It rewrites
  one set in place. A branch is scanned into it and taken back out, keeping
  only what it added, and the largest branch is left in place, so a chain of
  branches pays for its short arms (`rc.rs`, `Live`, `Scan::branches`). A
  `&&` asks an index which owned locals its right operand names, rather than
  walking the operand (`Scan::names`). An `else if` chain over 1,600 fields
  went from 610 M instructions in `middle` to 33 M, and §6.34's wide variants
  from 375 M to 320 M. `build::profile`'s
  `reference_counting_is_linear_in_a_long_branching_expression`.

The `build::profile` bounds read instructions off `BURI_PROFILE=1` and assert
nothing where the platform has no counter. `native::e2e::shapes` runs each shape
natively under the heap check and on JavaScript, and asserts only what it prints.

**Still worse than linear:**

- **`--release` on wide payloads and long tuples** grows 2.5 times per
  doubling. The derived functions' part of it is fixed in §6.34, and what's
  left is listed there.
- **`--release` on long matches and long templates** grows 2.4 times per
  doubling. §6.33 has both.
- **A debug build runs a match in time linear in its arms** where the decision
  tree falls back to a chain: `Int` and `Str` literal arms, or a variant split
  across two rows. LLVM turns the chain back into a `switch`.

### 6.33 Long matches and long templates in `--release`, 2026-10-06

Both of §6.32's open `--release` rows grow in `opt` and `llc`, not in the
emitter. The main unit's `opt -O2` and `llc -O2`, run on the IR the backend
hands them:

| Shape | `opt`, 200 → 400 | `llc`, 200 → 400 |
|---|---:|---:|
| match of 400 arms | 1.48 → 4.07 G | 1.02 → 1.75 G |
| template of 400 holes | 2.87 → 6.24 G | 4.01 → 9.79 G |

**A long match was ours.** The decision tree falls back to a chain of tag tests
for or-patterns, pairs and guards, and every block of the chain read the tag
again:

```llvm
b2:
  %slot5 = extractvalue { i16, i16 } %agg1, 0   ; a pair's element, again
  %tag6  = zext i16 %slot5 to i32               ; the tag, again
  %cmp7  = icmp eq i32 %tag6, 1
  br i1 %cmp7, label %b6, label %b5
```

`opt`'s first `SimplifyCFG` folds a chain into a `switch` only where each block
is a compare and a branch, so this chain reached InstCombine whole. InstCombine
asks every branch on a value about every compare of it
(`computeKnownBitsFromContext`, `foldICmpWithDominatingICmp`), which is `n²`:
3.8 times per doubling, 2.7 G of `opt`'s 4.1 G at 400 arms.

**The fix:** a tag or a field of a value in registers is read once per
function, right after the value is defined (`emit.rs`, `Unit::read_once`). The
chain is then foldable, and the or-pattern match costs `opt` 0.23 G at 400 arms
where it cost 0.68 G.

`buri build --release` of `long_match`, emit phase:

| Arms | Before | After |
|---:|---:|---:|
| 50 | 0.72 G | 0.68 G |
| 100 | 1.10 G | 1.18 G |
| 200 | 2.22 G | 2.04 G |
| 400 | 5.46 G | 4.07 G |
| 800 | 16.6 G | 9.19 G |

400 to 800 went from 3.0 to 2.26 times. At 100 arms the guarded match costs
`SimplifyCFG` 0.14 G more: a failed guard jumps back into the chain, which is
now a `switch`, and re-tests tags it already knows. The unit enum, payload
enum, wide payload, nested generic, enum chain, long tuple and records shapes,
and `cli/tests/example`'s two binaries, are within 1% of their old emit
phase. `native::llvm`'s `a_matchs_tests_of_one_value_read_its_tag_once` holds
the emitted IR to a few tag reads per function.

**What's left of a long match is LLVM's.** A pair's diagonal or a guard is `n`
compares of one value in `n` blocks, and InstCombine is `n²` in those whatever
shape surrounds them. A hand-written module in the best shape there is, a
`switch` on one value and a single compare of the other in each case:

| Cases | `opt -O2` | Per doubling |
|---:|---:|---:|
| 400 | 0.46 G | |
| 800 | 1.22 G | 2.7× |
| 1,600 | 4.12 G | 3.4× |
| 3,200 | 15.3 G | 3.7× |

At 800 arms that is 1.3 G of the 9.2 G.

**A long template is LLVM's register allocator.** `llc -time-passes`, 200 to
400 holes:

```text
Greedy Register Allocator   1.28 → 3.61 G
Live Interval Analysis      0.22 → 0.85 G
Instruction Selection       0.78 → 1.59 G
Machine Instruction Sched.  0.82 → 1.68 G
```

Every `Str` the program lets is three words, live until the template that
reads it, and `main` is 3,100 blocks, most of them the inline count
diamonds around the `incref`s and `decref`s. Live intervals and the greedy
allocator grow with values times the blocks they span. Two measurements:

- Templates of only `Str` holes grow 2.6 times per doubling, and of only `Int`
  holes 2.1 times.
- With every count a runtime call, `llc` grows 2.19 times per doubling and
  costs half as much. That's a call per count at run time, and `opt` took
  about 8 G more on the longer blocks, so it's not done.

`lower` makes it worse: it shows every hole before joining the first piece, so
all the converted strings are live at once too. Showing each piece's holes
next to its join would shrink the live set. It wouldn't change the growth,
since the program's own lets stay live.

### 6.34 A derived function reads a wide payload where it uses it, 2026-10-06

A derived `compare`, `==` or `hash` bound every payload field on entering a
variant's arm. At 200 fields that's 401 loads in a row before the first call,
and `llc`'s schedulers are quadratic in a region that long (§6.31).

Past eight fields (`derives.rs`, `EAGER_FIELDS_MAX`), the arm binds nothing and
the payload is read eight fields at a time, just before they're compared:

```text
match a { .V(..) => match b { .V(..) =>
    match a { .V(x0, …, x7, ..) => match b { .V(y0, …, y7, ..) =>
        <compare fields 0 to 7, and if they're equal:>
        match a { .V(.., x8, …, x15, ..) => …
```

One field at a time was worse. Each read tests both tags again, and `opt`'s
`JumpThreading` and `GVN` are quadratic in those tests. `opt -O2` plus
`llc -O2` on the derived `compare` of §6.32's wide variants alone:

| Fields | All at the arm | One at a time | Eight at a time |
|---:|---:|---:|---:|
| 100 | 2.99 G | 2.49 G | 1.46 G |
| 200 | 7.91 G | 5.99 G | 2.76 G |

`buri build --release`, emit phase, §6.32's shapes:

| Shape | 50 | 100 | 200 |
|---|---:|---:|---:|
| variants carrying `n` fields, before | 7.32 G | 18.00 G | 46.37 G |
| variants carrying `n` fields, after | 6.50 G | 14.87 G | 35.99 G |
| tuple of `n`, before and after | 3.25 G | 6.72 G | 15.30 G |

`middle` on the 200-field variants went from 0.14 G to 0.05 G, because `rc`
has fewer names live across the comparison. Debug emit is 0.16 G either way.

A struct or tuple was never affected: its derived functions read a field by
projection, where it's compared. `native::llvm`'s
`a_wide_variants_derived_functions_read_each_field_where_they_use_it` bounds the
longest run of loads in the derived functions at 32 and holds it flat from 100
fields to 200. `native::e2e::shapes` compares and hashes two variants that
differ only in their last `Int`.

**What's left on these two shapes isn't in the derives.** Each function's
`opt -O2` plus `llc -O2`, from 100 fields to 200:

- **`make`, which builds the variant**, goes from 1.3 G to 3.5 G.
  `JumpThreading` on it grows 5.6 times.
- **The derived `Show`** goes from 3.2 G to 7.2 G for the tuple. It's a
  template of 401 parts, and the greedy allocator's global splitting grows 3.1
  times.
- **`sum`, whose `let` binds all `n` fields**, goes from 0.74 G to 1.9 G.

### 6.35 The dev shell's cargo slowed every process start, 2026-10-06

On a quiet 12-core machine, a fresh target directory took 116 s to build and
224 s to test: 341 s against the five-minute budget. The test run used 1,052
CPU-seconds, 4.7 cores on average, so it wasn't bound by CPU.

**`DYLD_LIBRARY_PATH` was most of the CPU.** rust-overlay wraps cargo on
Darwin so a sandboxed build uses Nix's curl:

```sh
# /nix/store/…-rust-minimal-1.99.0/bin/cargo
DYLD_LIBRARY_PATH='/nix/store/…-curl-8.20.0/lib'$DYLD_LIBRARY_PATH
export DYLD_LIBRARY_PATH
exec "/nix/store/…-cargo-1.99.0-aarch64-apple-darwin/bin/cargo" "$@"
```

Every process under cargo inherits it: rustc, build scripts, test binaries,
and every `buri`, `clang` and `ld64.lld` a test starts. With it set, each start
costs far more CPU:

| Process | Without | With |
|---|---:|---:|
| `buri version` | 2.2 ms | 32.8 ms |
| `buri docs <id>` | 4.3 ms | 36.8 ms |
| `clang --version`, through the cc-wrapper | 38.6 ms | 167.1 ms |
| `ld64.lld --version` | 12.7 ms | 41.1 ms |
| `rustc --version` | 12.5 ms | 44.2 ms |

It hid well. A manifest-id shard took 0.3 s run by hand and 3.3 s under
nextest. Any runner script fixed it, because `/bin/bash` and `/usr/bin/time`
are SIP-protected, and macOS strips `DYLD_*` from what they start. That also
means `/usr/bin/time -l cargo …` measures the slow path, and a test binary
started from a shell measures the fast one. Part of §6.21's "half of
`ld64.lld` and 90% of the driver run before `main`" may have been this.

The dev shell now puts the unwrapped cargo first on `PATH` (`flake.nix`). A
shell isn't a sandbox, so it uses the system's curl, as rustup's cargo does.
`nix build` still uses the wrapped one, and CI uses rustup.

**The pool keeps seats with whoever holds them.** A worker that finishes a
case takes its next seat at once, and waiters ask every 10 ms
(`cli/tests/harness/pool.rs`). So a corpus that gets the seats keeps them
until it runs out of cases. `repositories::snapshots`, whose cases run for
seconds, got them first. The manifest-id shards, 2,000 processes of a few
milliseconds each, then held four test slots for 35–52 s, and the
rejected-program shards held seven more. `.config/nextest.toml` now starts
the cross test and the millisecond-case corpora first, and the manifest-id
shards take 3 s.

**The suite, alternating, tests and kept stores warm, load 11–55:**

| Run | Wall | CPU, user + sys |
|---|---:|---:|
| `main` | 177 s, 161 s | 1,081 s, 1,068 s |
| unwrapped cargo | 116 s, 132 s, 96 s | 584 s, 561 s, 563 s |
| unwrapped cargo and the new schedule | 99 s, 122 s, 96 s, 116 s | 563 s, 565 s, 558 s, 559 s |

The unwrapped cargo saves 500 CPU-seconds, 47%. At this load the schedule
doesn't separate from noise, because other agents held the cores. Its saving
is slots: 11 slots idle for about 40 s is 440 slot-seconds, or up to 37 s of
wall time on an idle machine.

**From a fresh target directory with no compiler cache**, building then
testing, load 13–25:

| Run | Build | Test | Total |
|---|---:|---:|---:|
| `main`, runtime archive stored | 164 s | 237 s | 401 s |
| this change, runtime archive stored | 65 s | 157 s | 222 s |

The `main` build is an outlier; two more builds of each side took 55–59 s and
52–71 s. A build whose environment hasn't built a runtime archive yet pays
`cli/build.rs`'s nested cargo on its critical path: 74 s, against 52 s for the
whole build without it. The store's key hashes the whole environment
(`hash_environment`), so the first build in a changed shell misses once,
including the first build after this change.

If the quiet run's ratio of wall time to CPU holds, an idle machine tests in
224 × 560 / 1,052 ≈ 119 s. With a 52–74 s build, that's 170–195 s.

**Measured and left:**

- **A wider pool.** Twice `available_parallelism` took 112 s and 119 s against
  105 s and 106 s at the default width, alternating at load 17–24.
- **A fair pool.** A turnstile lock in front of the seats made every worker
  queue, but handing the turnstile over is a wakeup. Seats then went out about
  100 times a second, and the manifest-id shards took 39 s instead of 3.
- **First-exec checks.** Three samples of `ui` cases each found a fresh
  `test-runner` still at `_dyld_start`, up to 4.6 s after launch, while macOS
  checked it. A fresh copy of a 0.5 MB program took 145 ms to run the first
  time and 31 ms after. Only fewer new executables per run would help, and
  that's `build/link.rs`'s job.
- **`buri-stencil`'s build script**, 2.6 s alone and 7 s in a cold build,
  where it delays `buri`'s compile by about 5 s. Its three targets build one
  after another, each split across `NUM_JOBS` compiles.
- **The hostile-schema tests**, 57 s each in the quiet run, took 0.2–2.3 s in
  every run here, fresh target directories included. That's a first-run cost
  on that machine, not the tests.

**nextest before 0.9.145 lent one test's pipes to another.** Two of ten runs
marked a test `LEAK` in the first seconds, though neither test leaves a child
behind: a pool test starts no process, and the manifest-id partition check
waits for its one child. The cause is the race `spawn::start_alone` guards
against in `buri`. macOS has no `pipe2`, so nextest's standard library makes
each test's capture pipe and marks it close-on-exec in two calls. A test that
nextest starts between them inherits the pipe and holds it until it exits, and
nextest reports the pipe's owner as leaky.

A runner that lists the descriptors each test process starts with shows it. Ten
bursts of 290 short tests each:

```sh
lsof -a -p $$ -d 3-254 -F fnt    # in the runner, before `exec "$@"`
```

| nextest | Tests started holding a pipe they didn't make | `LEAK` |
|---|---:|---:|
| 0.9.114, `nixos-25.11`'s | 4 | 1 |
| 0.9.146 | 0 | 0 |

0.9.145 makes the capture pipes itself, coordinated with spawning
(nextest-rs/nextest#3553). The dev shell builds 0.9.146 (`flake.nix`), and
`.config/nextest.toml` refuses anything older. A longer leak timeout wouldn't
have helped: the borrower holds the pipe for as long as it runs.

### 6.36 A derived hash is linear in a struct's fields, 2026-10-06

A derived `hash` threads one accumulator through every field:

```text
$mix($mix($mix(h, n), x.0), x.1) …
```

Inlining a field's hash copied its operands first, in case the inline failed
and a call needed them. The accumulator is one of them, so the `i`th field
copied the `i` fields before it. A struct or tuple of `n` fields was `n²` in
`middle`. A primitive's hash can't fail, so it now takes the operands
themselves (`derives.rs`, `Generator::at`).

`middle` on a tuple struct of `n` fields, alternately `Int` and `Str`, that
derives only `Hash`:

| Fields | Before | After |
|---:|---:|---:|
| 400 | 144 M | 7.6 M |
| 800 | 553 M | 12.2 M |
| 1,600 | 2,186 M | 23.0 M |

An enum variant wasn't affected. Since §6.34 its hash binds the accumulator
in a `let` every eight fields, so it never copied more than eight. §6.32's
wide variants read 85.8, 164 and 319 M in `middle` at 400, 800 and 1,600
fields, and 81.3, 157 and 303 M after. The derived `compare`, `==` and `Show`
build their chains outside the inlined call and were already linear.
`build::profile`'s `deriving_hash_is_linear_in_a_structs_fields`.

## 7. Profiling, on this platform

There is no `perf` on macOS and no hardware-counter dependency in the tree
(§3.2), so the way to turn a §6 gap into a function name is a sampling
profiler over the bench binary:

```text
cargo bench -p buri --bench compiler --no-run     # build it, find the binary path
samply record <bench-binary> --quick              # if samply is installed
xcrun xctrace record --template 'Time Profiler' --launch <bench-binary> -- --quick
```

`--quick` keeps the run short enough to profile. The phase timers dominate the
samples, so you can attribute the hot functions under `lex`, `parse`,
`Checker::run` and the lowering calls straight to their rows.

**A controlled sweep is the other instrument, and not the lesser one.** The
suite's whole parameter space is a command-line flag, so you can test a
suspected superlinear term by holding everything constant and moving the one
axis you suspect. `--param lines_per_module=2500` at a fixed `--scale` changes
the codegen unit count and nothing else that matters, and a row moving back to
its old rate under it says more than a flame graph: a profile says where the
time is, an experiment says what the time is a *function of*. That is how §6.4's
Θ(units × functions) finding came out.

**Two sweeps beat one.** A sweep over enum width once produced a curve that fell
and then flattened — suggestive, not an answer. A second sweep held the width
and moved the derive load, and the answer was a step function between one
derived trait and the next. An axis a single sweep leaves ambiguous is often two
axes, and the second sweep is cheap: both together were under ten minutes.

The two instruments are complements. §6.4's first finding is the case: the sweep
named the axis — the unit count — and the two suspects it made obvious were two
thirds of the cost; the third was a strongly-connected-components pass inside a
constructor, which nothing about the axis suggested and a profile pointed
straight at.

**macOS ships a profiler even where nothing is installed**:
`sample <pid> <seconds> 1 -file out.sample`, whose "sort by top of stack"
section is enough to read a self-time ranking. It is a poor substitute for
`samply` — no inverted call tree worth the name, and Rust's mangled symbols come
out raw — and it is much better than nothing.


---

## 8. Measuring on a noisy machine

**Compare instructions retired, not time.** On a shared ten-core M1 Pro, at
load 25–80 with two other agents compiling, one run gives an instruction count
within about 1% of any other run. Wall time swings by 77–97% at that load.

```text
/usr/bin/time -l buri test //...        # "instructions retired", no setup
BURI_PROFILE=1 buri test //...          # the same count, split by phase
```

### How steady each figure is

Ten runs of each, 2026-10-05. Range is (max − min) / median.

| Figure | Cold `buri test //...`, a 843-file monorepo | Cold `buri build` of one app |
|---|---:|---:|
| wall time | 71 s, range 77%, MAD 20% | 2.1 s, range 97%, MAD 7.7% |
| CPU time | 19.2 s, range 5.3%, MAD 1.2% | 1.49 s, range 9.7%, MAD 2.2% |
| cycles | 54 G, range 5.7%, MAD 1.0% | 4.3 G, range 10%, MAD 1.7% |
| **instructions retired** | **135.5 G, range 0.79%, MAD 0.17%** | **10.0 G, range 0.85%, MAD 0.19%** |
| allocations | | 8.03 M, range 0.13% |

Per phase, from `BURI_PROFILE` over eight to ten more builds:

| Phase figure | Range |
|---|---:|
| instructions, each phase | 1.0–1.8% (link 5.8%) |
| allocations, single-threaded phases | 0.00% |
| allocations, parallel phases | ≤ 0.14% |
| CPU time, each phase | 8–34% |
| wall time inside a phase | 15–234% |

Instructions aren't exact, for two reasons:

- **Short processes have a one-sided tail.** Ten runs of the bench's `sema`
  child on `saved:mixed-10k` read 187.4–188.1 M, plus two at 189.9 and
  192.3 M. Startup and the kernel probably add work and never remove it, so
  take the **minimum** of two runs for anything under a second.
- **Parallel runs wobble both ways**, by about ±0.4%, likely from lock
  spinning, the allocator and how the pool divides work, which all move with
  scheduling. The median of two runs is the reading there.

Allocations are exact wherever one thread does the work, which makes them the
sharpest signal for a front-end change.

### `BURI_PROFILE=1`

Set it on any command and the toolchain prints, at exit:

```text
buri profile           instructions        cpu       busy  child instructions  child cpu
  lex+parse               5691.6 M    0.499 s    1.228 s               0.0 M    0.000 s
  check                  13727.6 M    2.068 s    6.192 s               0.0 M    0.000 s
  monomorphize           11520.6 M    1.542 s    6.623 s               0.0 M    0.000 s
  middle                 11404.1 M    2.204 s    7.699 s               0.0 M    0.000 s
  emit                   55468.4 M    6.268 s   22.587 s               0.0 M    0.000 s
  link                    7163.6 M    1.540 s    9.305 s           15743.9 M    2.214 s
  run                      280.4 M    0.113 s  542.070 s         1237788.9 M  157.069 s
  other                  27494.5 M    4.730 s  458.686 s               0.0 M    0.000 s
  all phases            132750.7 M   18.965 s 1054.389 s         1253532.8 M  159.283 s
wall 105.049 s
process: 135.920 G instructions, 19.646 s cpu, 66.326 s runnable but not running, peak 1814 MB
child processes: 174.066 s cpu
```

- **instructions** and **cpu** are summed over every thread while it was in
  the phase. Each thread reads its own counters (`thread_selfcounts` on macOS,
  no root or entitlement needed) whenever its phase changes, so phases that run
  side by side still split correctly. Workers start in their spawner's phase.
- **busy** is wall time summed over threads, so for `link` and `run` it's
  mostly time spent waiting on a child.
- **child instructions** and **child cpu** are the linkers, test binaries and
  generators a phase waited for, read from the exited child before it's reaped.
  Generators count under `other`.
- **allocations** appears when the binary is built with
  `--features alloc-counter`, which costs a thread-local increment per
  allocation.
- The **process** line is the kernel's whole-process total. It's above "all
  phases" because threads that never enter a phase, such as pipe readers,
  aren't in the table. **runnable but not running** is how long threads waited
  for a core: the direct measure of how loaded the machine was.

Off, a phase change costs one load of a flag. Ten builds with profiling off
read 10.03 G instructions against `main`'s 10.02 G, inside the noise. On Linux
the instruction column reads `-` and the child columns don't appear: there's
no per-thread counter without `perf_event_open`.

That run says something no timer had: **the toolchain is a tenth of the CPU a
cold `buri test` costs.** The suites themselves are 157 of the 174 child
seconds, and the linker is 2.2. A cold `buri build` of one app there is the
same story smaller: the compile is 1.5 s of CPU, and the generators it runs
under `bun` are 5.1 s.

### The protocol

```text
BURI_PROFILE=1 ./a/buri test //... 2> a1.txt
BURI_PROFILE=1 ./b/buri test //... 2> b1.txt
BURI_PROFILE=1 ./a/buri test //... 2> a2.txt   # only if the delta is under 3%
BURI_PROFILE=1 ./b/buri test //... 2> b2.txt
```

Clean before each run when you're measuring a cold build; `buri clean` is its
own process and stays out of the figures.

- **One run each** settles any delta of 3% or more, overall or in one phase
  (6% for `link`, which waits on the file system).
- **Two alternating runs each** settle 1%: a delta is real when the two ranges
  don't overlap. Ten builds of `main` against a toolchain with only profiling
  added gave ranges of 9.98–10.06 G and 9.96–10.05 G, which is what no change
  looks like.
- **Under 0.5% is noise** at any count of runs this protocol allows.
- Below a second of work, compare minimums rather than medians.
- For synthetic corpora, `cargo bench ... -- --rss` gives the same count per
  phase from the bench's one-phase children (§4, "Peak memory").

Then confirm the winner's wall time once on a quiet machine, because a change
can retire fewer instructions and still run slower.

### What counts can't see

- **Waiting.** Lock contention, I/O and a thread idle for want of work don't
  retire instructions. Compare `cpu` with `busy` per phase: a phase whose busy
  time far exceeds its CPU time is waiting on something.
- **Lost parallelism.** Instructions don't drop when a pass goes parallel; wall
  time does. Read **wall** against **process cpu** and **runnable but not
  running**: wall near cpu ÷ cores is a parallel run, and runnable time says
  how much the machine's load, rather than the code, cost.
- **Launch checks.** `syspolicyd` scans a fresh binary before its first `exec`.
  It lands in `busy` on `run` with no CPU behind it, mixed in with everything
  else a test waits for. There's no counter for it: measure it on a quiet
  machine, first run against second run of the same binary.
- **Cache and branch behavior.** Cycles see these, and cycles move 6–10% with
  load. A change that mostly helps memory traffic needs §7's profiler and a
  quiet machine.

### Dead ends

- **`perf stat` in a Linux container.** This machine has `podman`. Its VM
  exposes no hardware PMU (only `software`, `tracepoint` and probe event
  sources), so `perf stat -e instructions` is unsupported even privileged.
- **Cachegrind in the container works and is exact**: 269,464 instructions
  for `ls /`, twice. It needs a Linux build of the toolchain and simulates
  every instruction, so it's far slower, which buys exactness below the 0.5%
  floor. Nothing here needs that yet; it's the tool to reach for when
  something does.
- **kpc and per-thread cycle counters beyond the fixed two** need root.
  `thread_selfcounts` gives instructions and cycles, which is all this needed.
- **Process deltas at phase boundaries.** `proc_pid_rusage` is per process, and
  `buri test` checks one suite while it links another, so a phase boundary
  isn't a moment in time.
