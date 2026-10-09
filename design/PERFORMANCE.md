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

All three hold on `mixed-1M` on an M3 Pro: parse at 1.27×, check at 4.19× and a
cold debug build at 3.93× (§6.82). Two CI jobs fail when one stops holding, by
instructions a line and by the clock. `cli/benches/compiler.rs` is what says so.

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
  --goals=<mode>    count | wall: the goals gate (§6.82)
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
- One proto `generate` over all 16, and one `//tools/app_manifest` `generate`.

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

400 to 800 went from 3.0 to 2.26 times. At 100 arms the guarded match cost
`SimplifyCFG` 0.14 G more: a failed guard jumped back into the chain, which is
now a `switch`, and re-tested tags it already knew. The unit enum, payload
enum, wide payload, nested generic, enum chain, long tuple and records shapes,
and `cli/tests/example`'s two binaries, are within 1% of their old emit
phase. `native::llvm`'s `a_matchs_tests_of_one_value_read_its_tag_once` holds
the emitted IR to a few tag reads per function.

**A failed guard now skips the arms it can't match** (`lower.rs`,
`FnLower::chain`). It goes on at the first later arm whose pattern isn't
disjoint from its own, so `.V7 if k > 7` fails straight to the catch-all.
`native::llvm`'s `a_failed_guard_skips_the_arms_it_cannot_match` holds this.
Emit phase of `--release`, for `long_match` and for its `guard` and `nth`
alone:

| Arms | `guard` alone, before | After | `long_match`, before | After |
|---:|---:|---:|---:|---:|
| 100 | 0.40 G | 0.23 G | 1.18 G | 1.01 G |
| 200 | 0.72 G | 0.40 G | 2.04 G | 1.73 G |
| 400 | 1.48 G | 0.76 G | 4.07 G | 3.35 G |
| 800 | 3.32 G | 1.65 G | 9.18 G | 7.53 G |

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

`lower` made it worse: it showed every hole before joining the first piece, so
all the converted strings were live at once too. Now each piece's holes are
shown next to that piece's join (`FnLower::template_pieces`), and
`native::llvm`'s `a_long_templates_holes_are_converted_beside_their_join`
holds it. On a template of 300 `Int` holes, `llc -O2 -time-passes`:

```text
Greedy Register Allocator   229 → 85 M
Live Interval Analysis       21 → 6 M
Total                      1.52 → 1.29 G
```

`long_template`'s emit phase under `--release`:

| Holes | Before | After |
|---:|---:|---:|
| 100 | 2.75 G | 2.70 G |
| 200 | 6.40 G | 6.12 G |
| 400 | 15.3 G | 14.4 G |
| 800 | 36.9 G | 34.5 G |

The growth doesn't change, since the program's own lets stay live.

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
  §6.38 is one that hands over only when another process waits.
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

### 6.37 A debug build's emission is linear in a variant's fields, 2026-10-06

§6.32's wide variants still grew 2.5 to 2.9 times per doubling in a debug
build's `emit`. Two loops in the stencil backend were `n²` in a value's fields:

- **A field read typed the whole value.** `GetField` and `GetPayload` asked
  `types::field_types` or `variant_types` for every field's type, to check
  whether the one being read is boxed. The derives and the matches read each
  of `n` fields, so that was `n` lists of `n` types. They ask for the one field
  now (`jit.rs`, `Jit::field_ty`).
- **Building a value scanned back from each field.** A field computed just
  before the `MakeEnum` or `MakeStruct` that stores it shares the value's slot
  if nothing between the two touches the value. That was a scan of the
  instructions in between, per field. `alias_parts` now records, per value
  built in a block, the last instruction before it that touches it, in one
  pass.

`buri build` of §6.32's wide variants, `emit` phase:

| Fields | Before | Slots alone | Both |
|---:|---:|---:|---:|
| 400 | 358 M | 322 M | 246 M |
| 800 | 903 M | 755 M | 461 M |
| 1,600 | 2,665 M | 2,054 M | 891 M |

`build::profile`'s `a_debug_builds_emission_is_linear_in_a_variants_fields`
builds a variant of 1,000 and 2,000 fields with every derive. With only the
slot fix it grew 2.49 times.

### 6.38 A fair pool, 2026-10-06

§6.35 left the pool unfair. A worker that finishes a case takes its next seat
at once, and a waiting thread asks every 10 ms, so a corpus that gets the
seats first keeps them. `hermeticity::hermeticity_rules`, 1 s alone, took
45–72 s in a full run behind `repositories::snapshots`.

Now a thread takes no seat while another process holding fewer seats waits
(`cli/tests/harness/pool.rs`):

```rust
held > 0 && lock::held_by_another(&self.queue, 0, (held as i64) << 32)
```

A waiting thread holds an `fcntl` read lock on one byte of a `queue` file, at
`held << 32 | slot`, where `held` is its process's seat count. Those locks fit
for three reasons:

- `F_GETLK` asks whether anyone poorer waits in one call. Nobody waiting is
  the common case, and it costs that one call and no handover.
- They belong to the process, so a process never defers to its own threads.
- The OS drops a dead process's locks, so a crashed waiter leaves nothing
  behind. Seats are still `flock`s, which the OS drops too.

The process waiting with the fewest seats is never told to wait, so nobody
starves, and the processes that want seats split them evenly, give or take
one. §6.35's turnstile queued every hand-out behind a 10 ms poll. Here only a
seat that changes process waits for one.

`pool_tests::a_waiting_process_gets_the_next_seat_a_busy_one_frees` runs the
test binary again as a busy process that fills a two-seat pool, and counts the
seats it takes back while the test waits. The old pool took back 20 of 20,
this one none.

**A small corpus started 8 s after `repositories::snapshots`**, running the
test binary by hand (`hermeticity_rules`):

| Pool | Alone | Behind `snapshots` |
|---|---:|---:|
| `main` | 1.1 s | 32.0 s, 68.8 s |
| fair | 2.8 s | 3.9 s, 0.6 s |

**The suite, alternating, load 18–43:**

| Pool | Wall | CPU, user + sys | `hermeticity_rules` | Manifest-id shards |
|---|---:|---:|---:|---:|
| `main` | 138 s, 108 s, 130 s | 555 s, 554 s, 564 s | 72 s, 45 s, 62 s | 1.2–1.8 s |
| fair | 108 s, 105 s, 106 s | 551 s, 556 s, 556 s | 1.2 s, 1.1 s, 0.5 s | 2.6–2.9 s |

`repositories::snapshots` now shares its seats, so it went from 54–74 s to
80–92 s, still well inside the run. The manifest-id shards lose about 1.4 s to
seats waiting a poll on their way between processes.

Without the manifest ids' head start in `.config/nextest.toml`, they took
5–7 s, against 35–52 s in §6.35. The suite read 140 s and 98 s, against 126 s
and 106 s with it, which is noise, so the head start stays.

### 6.39 Every new code file waits in one system-wide check, 2026-10-06

In the `ui` corpus, 183 new `test-runner`s per run waited a median 5.7 s each
at `_dyld_start`, 954 s in all. A runner that is already checked and loads
each suite some other way would skip that, if the other way skips the check.
None that loads a file does.

**Measured:** one already-run host loads fresh, distinct images, alternating
kinds, a new host process per load. Each image is
`clang` output with its own seed and 5 MB of new bytes in `__TEXT`. Load
12–25:

| Fresh 5 MB image | First load, median | Min | Second load |
|---|---:|---:|---:|
| executable, `posix_spawn` | 216 ms | 213 ms | 1.6 ms |
| dylib, `dlopen` | 221 ms | 212 ms | 0.2 ms |
| bundle, `dlopen` | 218 ms | 213 ms | 0.2 ms |
| executable, `codesign -f -s -` again | 217 ms | 208 ms | 1.6 ms |
| dylib, `codesign -f -s -` again | 215 ms | 210 ms | 0.2 ms |

- **A dylib waits where an executable does.** `sample` finds `dlopen` in
  `dyld4::Loader::mapSegments` → `fcntl` for the whole 200 ms. `/usr/bin/time
  -l` reads 0.23 s real and 0.00 s user and system.
- **The check is per file, not per bytes.** A `cp` of a dylib that had
  already been checked pays it again.
- **`mmap(PROT_READ | PROT_EXEC)` of a fresh file pays it too**, then fails
  with `EPERM`. A `dlopen` of that file afterwards takes 0.2 ms.
- **Size matters little.** 0.5 MB took about 200 ms, 5 MB 230 ms, 20 MB
  324 ms, for either kind.
- **The checks queue system-wide.** 24 fresh images, 12 at a time, took a
  median 1.5 s each and 5.7 s in all, about 230 ms per image either way. So
  `ui`'s 5.7 s median is queueing behind other checks, not a slow check.
- **Code with no file behind it skips the check.** 5 MB copied into `MAP_JIT`
  memory and called took 1.2–1.8 ms. Using it means loading each
  suite's objects in-process: relocations, binding to the runtime, thread
  locals and unwind info. That's a linker, so it waits for a decision.

**One runner per run doesn't cut files.** `ui` runs one suite per step: 294
of its 296 `buri test` runs name one target, and an exec log shows every
fresh runner at the shared `test-runner`, one at most per run. Runs that touch
several suites already share binaries (`commands/test.rs`, `batch`).
`cli/tests/example` makes two files from eight suites, because `server`
forbids `client` and a batch splits on tags:

| Step in `cli/tests/example` | Fresh files |
|---|---:|
| cold | 2 |
| unchanged | 0 |
| a new test in `//lib/money` | 1 |
| a change to `//lib/money`'s code | 2 |
| `--force` | 0 |

Only 25 runs in every `repositories` corpus could touch more than one suite.
Fewer new files would have to come from fewer runs, not wider ones.

### 6.40 The stencil emitter's own overhead, 2026-10-06

A debug build's `emit` spent about 5,400 instructions per stencil it copied.
Copy-and-patch should cost a few hundred. Most of the rest was bookkeeping
around the copy: allocations, `core::fmt`, and the same operand walk done five
times per function. Eight changes in `crates/stencil`, each its own commit:

- **Keys on the stack.** Every emitted instruction built its key with
  `format!`. `key!["bin/", name, "/", tag]` copies the pieces into an inline
  `jit::Key`, and `Loc::tag` borrows. -6.5%, then -2.4% for dropping `fmt`.
- **One read count per function.** Coalescing, call pinning, aliasing, the
  register allocator and constant folding each counted every value's reads.
  They share one count. -5.0%.
- **Tables refilled, not reallocated.** A function's `Fn2` and every
  analysis's side tables live in the worker's `Scratch` (`jit::Bufs`) and are
  cleared and refilled per function. -10.7%.
- **Runtime calls without allocation.** `c_call_to` built each argument's hole
  name with `format!` into two vectors. The names are a static table and the
  bindings an array. -5.0%.
- **Aliasing without hash maps.** `alias_parts` built two maps per block and
  typed every field of a value. The maps are per-function tables stamped with
  the block, and a field is typed only when it is still a candidate. -3.4%.
- **One operand table.** `Inst::operands` ran in five passes. `jit::Operands`
  lists every row once per function. -2.3%.
- **The runtime table indexed.** `runtime_table::entry` scans 300 rows, once or
  twice per runtime call. `stencil::runtime::entry` asks an index. -0.6%.

`BURI_PROFILE=1 buri build`, cold, `emit` instructions, the lower of two
alternating runs:

| Workload | Before | After | |
|---|---:|---:|---:|
| `mixed-10k` × 8 binaries | 1,492 M | 1,046 M | -30% |
| eight saved corpora and ten shapes, 18 binaries | 717 M | 520 M | -27% |
| §6.32 wide payload, 200 | 129 M | 98 M | -24% |
| §6.32 long match, 400 | 87 M | 64 M | -26% |
| §6.32 payload enum, 400 | 234 M | 181 M | -23% |
| §6.32 records, 200 | 110 M | 85 M | -22% |
| §6.32 long tuple, 400 | 125 M | 96 M | -23% |
| `cli/tests/example` server | 17.0 M | 16.6 M | -2% |

Allocations in `emit` for `mixed-10k` × 8 fell from 1.45 M to 0.66 M before the
last three changes. The bytes didn't move: every object the 18-binary
repository emits, every object of the eight saved corpora for all three
targets (270, emitted in-process, so the Linux ones need no cross runtime), and
all three stencil libraries are identical before and after.

**`build.rs` compiles from one queue.** It built the three libraries one after
another, each on its own pool, and waited three times for a slowest shard. The
probes now run side by side, all thirty shards share one queue of `NUM_JOBS`
workers, and the libraries assemble side by side. A cold run into an empty
`OUT_DIR` at load 24: wall 3.48 s to 2.93 s, median of three, CPU unchanged at
20 s. Under a loaded cold `cargo build` the script is CPU-bound, so this helps
the uncontended case most.

**Measured dead ends:**

- **A per-worker cache of key lookups**, 256 slots in front of the library's
  index. Instructions and `emit` CPU didn't move.
- **Hole names packed into words** to skip `memcmp` when binding. Packing every
  binding cost more than the compares: +7.7%.
- **A per-`TypeId` aggregate-size cache** in front of `Layouts::shared`. Under
  1%.

**A second round** took `emit` down another 16%. Seven changes, each its own
commit:

- **Literal keys by address.** `mixed-10k` × 8 copies 280,000 stencils under
  79 distinct keys, and half are literals like `"jump"` or `"decref/free"`. A
  literal's address never changes, so a worker maps it to its library index in
  a 256-slot table. `arm_key` already had the index, so it passes it on, and
  the width-keyed moves name literals instead of spelling `mov/8`. -2.4%.
- **Binding without `memcmp`.** Every hole name starts with `JIT_`, so each
  equal-length pair went to `memcmp`. `same_name` compares from the end,
  inline. -2.7%.
- **Flat loop graphs.** `promote` runs on every function whose `match` joins,
  because `lower` lays the join out ahead of its arms. It built predecessors,
  successors and the dominator tree as a `Vec` per block, fresh per function.
  `Lists` keeps them in two flat tables refilled from `Bufs`. -4.8%.
- **Imports borrow their names.** `import` cloned the hole's name for every
  relocation. The library lives for the process, so `Target::Symbol` is a
  `Cow<'static, str>`. -3.4%.
- **Latch classes as rings.** `coalesce_latches` kept a `Vec` per slot class.
  A ring through one `next` table merges two classes with a swap. -1.6%.
- **`frame_sigs` shares `Cycles`** and sizes aggregates by `TypeId`. It runs
  on the main thread before the workers start. -1.1%.
- **Relocations remember their symbol.** `assemble_unit` hashed
  `buri$stencil$pool` for both halves of every pool reference, and the
  callee's name for every call. -1.7%.

On `dc96906ac`, cold, `emit` instructions, the lower of two alternating runs:

| Workload | Before | After | |
|---|---:|---:|---:|
| `mixed-10k` × 8 binaries | 1,054 M | 884 M | -16% |
| eight saved corpora and ten shapes, 18 binaries | 521 M | 437 M | -16% |
| §6.32 wide payload, 200 | 98 M | 80 M | -19% |
| §6.32 long match, 400 | 64 M | 60 M | -6% |
| §6.32 payload enum, 400 | 181 M | 151 M | -17% |
| §6.32 records, 200 | 85 M | 69 M | -19% |
| §6.32 long tuple, 400 | 96 M | 78 M | -19% |
| `cli/tests/example` server | 16.2 M | 16.2 M | 0% |

The bytes didn't move: the 18-binary repository's objects and the 270 objects
of the eight corpora on all three targets are identical before and after.

**Second-round dead ends:**

- **Sizing each part's region from the last one** to skip `Vec` doubling.
  -0.5%.
- **Borrowing the object writer's section bodies** instead of copying them,
  with relocations sorted as `(section, offset, index)` tuples. Together
  they cost +1.5%, and the borrow alone didn't move.

**What's left:**

- About 3,150 instructions per stencil, spread thin. `Jit::emit` is about 30%
  of a worker's samples, and pairing holes with bindings is still 5%. The slot
  analyses are 16%, `assemble_unit` about 14% of the phase, and none of them
  has a single hot spot.
- **Dropping the lowered program** is 5% of the phase, on the main thread after
  the workers finish. Most of it is `ir::Program`'s many small allocations,
  which live in `crates/middle`.
- **A shared store for the shard objects.** Each fresh worktree pays the 20 s
  of `cc`. The shard cache in `OUT_DIR` keys on the generated C alone. A store
  under `~/.buri/toolchain-build/` would need the C, the flags, the
  `cc -###` line with its paths made portable, and the resolved `clang`'s
  identity, as §6.24 asks.

### 6.41 The formatter and the checker, 2026-10-06

Instructions per repetition of each phase, from a bench child that runs it once
and then four times, `main` at `1c674296` against the seven commits below. `check`
is the `sema` child minus the `load` child, so parsing drops out.

| Corpus | `check` before | after | Δ | `format` before | after | Δ |
|---|---:|---:|---:|---:|---:|---:|
| `mixed-100k` | 470 M | 438 M | −6.8% | 2,237 M | 1,642 M | −27% |
| `match-heavy-100k` | 550 M | 440 M | −20% | 2,073 M | 1,493 M | −28% |
| `enum-heavy-100k` | 655 M | 414 M | −37% | 2,056 M | 1,529 M | −26% |
| `mixed-few-files-100k` | 489 M | 454 M | −7.2% | 4,457 M | 1,662 M | −63% |
| `string-heavy-100k` | 457 M | 443 M | −2.9% | 2,314 M | 1,684 M | −27% |
| `comment-heavy-100k` | 267 M | 252 M | −5.6% | 1,753 M | 1,253 M | −29% |

`lex+parse` didn't move: every row is within 1.7%, both ways.

**The formatter was `n²` in a file's length, three ways.** Every lookup of the
comments above a token scanned the file's whole list, `written_after` scanned
the declarations once per declaration, and `derive_groups` compared every
declaration with every other. Comments are in source order, so lookups binary
search, the declarations' starts are listed once, and derives group through a
map. `buri format` of one file from `/tmp/buri-algo/gen.py big_file`:

| Declarations | Lines | Before | After |
|---:|---:|---:|---:|
| 1,000 | 27k | 3.50 G | 0.43 G |
| 2,000 | 54k | 12.92 G | 0.81 G |

`build::profile`'s `formatting_is_linear_in_a_files_comments` formats 2,000
and 4,000 commented functions. Before, it read 35.3 G and 277.2 G.

**Then the formatter's constant**, each change measured on `mixed-100k`
against the one before:

| Change | `format` |
|---|---:|
| One lex per text: the parse hands its tokens and trivia to the comments and the check (`parse_kept`) | −12.8% |
| `Doc::Text` is a `Cow<'static, str>`, so a `,` or a keyword isn't an allocation | −4.5% |
| `Doc` borrows identifiers, numbers and verbatim lines from the source | −2.2% |
| `fits` measures on one spare list per render, and `Alt` candidates are an iterator | −5.9% |

A file used to be lexed five times: to parse it, for its comments, for the
comment check, and twice more for the output. Now it's twice. The parser
reads a token's doc lines from the trivia table instead of taking them, which
is what lets the table be read again.

**Most matches skip the usefulness matrix.** `plainly_covered` takes a match
whose arms are distinct variants or literals binding only names, then at most
one catch-all. Every arm of that shape is reachable and exhaustiveness is a
count, so the matrix would report nothing. Any other match goes the long way,
including every one with something to report, so no diagnostic changes.

**A local lookup follows an index past the innermost 32 bindings.**
`lookup_local` walked every binding in scope, and a name that isn't a local,
such as a function's, walked all of them. Older bindings are now indexed by
name hash, each pointing at the one it shadows. Short bodies still walk their
list, and `mixed-100k` pays about 1% of `check` for the bookkeeping:

| `check` of one body of `n` `let`s | Before | After |
|---|---:|---:|
| 8,000 `let`s calling a function | 270 M | 78 M |
| 16,000 `let`s calling a function | 890 M | 127 M |
| `gen.py path_lets`, 8,000 | 1,277 M | 130 M |
| `gen.py closures`, 4,000 | 508 M | 127 M |

`build::profile`'s `checking_is_linear_in_a_bodys_lets` guards it.

Formatted output is identical on all 7,787 checked-in `.buri` files, which
include the unformatted inputs under `cli/tests/formatting`, and on six pinned
corpora. The full workspace suite passes.

**Measured and dropped:**

- **Scanning a word eight bytes at a time** in the lexer, SWAR: +4% `lex`
  instructions. A word is short, so the byte loop retires fewer.
- **Sizing a module's `names` map for the prelude up front**, and **one
  `resolve_path` per named type** in `elaborate` instead of two: both within
  noise.

**What's left.** Dropping the typed tree is about a tenth of a `sema`
repetition, freeing one `Box<Expr>` at a time. An arena would fix it, and
`middle` reads that tree. `ModuleScope::names` copies every prelude name into
every module as a `String`, and the language server iterates it. The `Doc` tree
is still a `Box` or a `Vec` per node, and freeing it is about a tenth of
`format`.

### 6.42 The middle end's own overhead, 2026-10-06

A scratch harness ran each pass of `monomorphize::run`, `middle::run`,
`middle::native` and `lower::run_with` on the bench's generated corpora, and
read the process's instructions retired between passes. Best of three, 100k
lines, `main` at `526fe94bc`:

| Corpus | `main` | After | Δ | `monomorphize` | `rc::analyze` | `lower` |
|---|---:|---:|---:|---:|---:|---:|
| `mixed` | 1,712 M | 1,425 M | −16.8% | −41% | −10% | −20% |
| `generic-blowup` | 2,501 M | 2,018 M | −19.3% | −44% | −11% | −25% |
| `enum-heavy` | 2,526 M | 2,239 M | −11.4% | −44% | −11% | −8% |
| `derive-heavy` | 1,641 M | 1,391 M | −15.2% | −39% | −10% | −18% |
| `match-heavy` | 1,365 M | 1,134 M | −17.0% | −44% | −11% | −19% |
| `struct-heavy` | 846 M | 698 M | −17.5% | −37% | −11% | −23% |

Most of it was work done again for an answer already in hand:

- **`monomorphize`.** `canonical_ty` rebuilt every substituted type level by
  level, interning each, to swap context ids that almost never move. It reads
  first now, and skips the walk when every context is its own canon.
  Every instantiation spelled its declaration's symbol a character at a time
  and formatted its type arguments into a `String` to hash. The base symbol is
  kept per declaration, each argument's spelling per type, and the tag is
  FNV-1a over the same bytes, fed in pieces. `build_fn` cloned the whole
  `FnInfo` per instance and `descriptor` the whole `TyCon`; both borrow
  through `&'a Checked` now.
- **`lower`.** `Sites::of` hashed every node's address, where only the nodes
  the plan names are ever asked for. Each function's type interner rendered a
  name for every type it met, and the merge kept one; the merge names a type
  when it adopts it. `Units::of` parsed every debug name a second time.
- **`rc`.** `scan_func` walked each body four times before scanning it, for
  subtree sizes, jumps, names and `let` bindings. `BodyIndex` is one walk, and
  the name index is by local rather than hashed. `consuming_uses` redid its
  growth scan every round and ran one round past its last growth. It stops
  once no `let` or `match` that kept nothing could keep something now.
- **Smaller.** `dce` hashed every symbol with SipHash. `derives` cloned a
  descriptor per body; the table is behind an `Rc`. `inline` walks a body only
  when it calls something inlinable.

Two shapes from the algorithm audit were quadratic. `forward` walked a block's
whole subtree for update bases at every nested block, and walked each later
statement once per forwarded path. It's one walk per body now, with the paths
in a table. `closures` handed each lifted lambda a copy of its parent's whole
table of locals, which then sized every later per-function pass. A lifted
function gets a table of what its body names. `middle` in a debug native
build:

| Shape | n | `main` | After |
|---|---:|---:|---:|
| field-reading `let`s in one body | 2,000 | 807 M | 38 M |
| | 4,000 | 3,147 M | 73 M |
| | 8,000 | 12,427 M | 143 M |
| capturing lambdas in one function | 1,000 | 223 M | 61 M |
| | 2,000 | 746 M | 121 M |
| | 4,000 | 2,269 M | 235 M |
| `if` blocks nested `n` deep | 64 | 99 M | 55 M |

`build::profile`'s `forwarding_field_reads_is_linear_in_a_bodys_lets` and
`closure_conversion_is_linear_in_a_functions_lambdas` grew 3.76 and 3.59
times per doubling before.

End to end, two alternating runs each:

| Run | Phase | `main` | After |
|---|---|---:|---:|
| `buri build` of `saved:mixed-10k`, native | `monomorphize` | 33.2–33.8 M | 20.8–21.7 M |
| | `middle` | 95.9–98.3 M | 88.2–90.8 M |
| | process | 0.835–0.919 G | 0.805–0.886 G |
| `buri test //...` in `cli/tests/example` | `monomorphize` | 42.4–43.4 M | 25.0–26.1 M |
| | process | 1.090–1.092 G | 1.073–1.078 G |

Output is identical. The JavaScript, the `rc::Plan`'s `Debug`, the IR's
`Display` and every stencil object hash the same for 14 generated profiles at
3k and 30k lines. So do the `saved:mixed-10k`, `cli/tests/example` and
1,000-lambda and 2,000-`let` executables, and both suites pass.

**Tried and dropped.** Reading each body's effects in the walk that builds
the ownership graph saved a walk and no instructions: the per-node match is
the cost, not the traversal. Measuring a pasted body from its callee's measure
instead of walking it cost 2–6% more in `inline`, because most pasted bodies
fold, and a fold still needs the walk.

**What's left.** `rc::Syntactic::new` walks every body three times per native
build: in `middle::native`, in `lower`'s list loops, and in the backend.
Handing the first to `lower` needs a field on `rc::Plan`, and a test outside
the middle end builds one literally. `Scan::expr`'s sets are hashed by
`LocalId`, and their iteration order reaches the tick order a branch scan
reads, so dense sets need care. Dropping the program is 7% of the table.

### 6.43 A build with nothing to do, 2026-10-06

A warm `buri build //...` of a generated monorepo (200 libraries of four
modules, 6 native and 6 node binaries, `/tmp/buri-perf-driver/gen.py`) took
17.9 G instructions and a second to find that nothing had changed. Most of it
was work a cache already held the answer to.

- **A module path finds its package by lookup.** `resolve_module` tried every
  package longest first and built `format!("{pkg}/")` for each, per import.
  Only the path, its prefixes before a `/` and the root can own it, so those
  are looked up. Lex+parse 14.0 G → 2.2 G.
- **A warm native build skips the front end.** A native artifact is keyed on
  its IR, so a no-op build still checked, lowered and hashed the whole
  program. A record keyed on the JavaScript artifact's key, the build graph
  and the linker names the `link` key and each unit's `codegen` key. A hit
  places that executable and prints the same `--explain` lines.
- **A clean library check is remembered.** `buri build //...` analyses every
  library it names. A check with no diagnostics is recorded under the graph
  and its closure's sources, each member's part hashed once per command. One
  with any diagnostic is never recorded, so it prints every time.
- **Packages are found from the listing.** `collect_packages` asked `is_dir`
  of every entry. The listing's file types answer, and a symlink is still
  followed. About -1.4%.
- **One batch of reads per key.** `contribute_as` started a `parallel::map`
  per closure member: 250 threads to read 250 small files. All members are
  read in one batch, on one thread below 32 files. Key bytes are unchanged.
- **One read per source per command.** `buri build` and `buri test` without
  `--watch` keep what their keys read for the life of the process
  (`actions::remember_reads`). A watching process never turns it on.
- **The next link takes over the last link's directory.** A link ran in
  `.buri/link/<link-key>`, and the key moves with any unit, so one edit
  hard-linked every object into a fresh directory, about 0.5 ms each on APFS.
  The directory is now named for the output. A link renames it to its private
  name, keeps the objects it still names and drops the rest, so unchanged
  objects are already the cache's files. An incremental link and a `--force`
  link of 800 modules are byte-identical.

**A stale hit, fixed.** A package's modules that no rule lists are importable
(`unused-source` is a lint), but the key is built from `rule_files` before
anything loads. Editing one left a JavaScript artifact built from its old
bytes. Each entry now records the files its analysis read that the key didn't
hash, with digests (`actions::unkeyed_reads`), and a hit holds only while they
match. The native record and the library check record do the same.
`incrementality::an_import_no_rule_lists_cannot_serve_a_stale_answer` covers
it.

`buri test` and tools had the same gap. A suite's verdict and recorded build
now carry what its load read that the key didn't hash: an unlisted module, or
a library it imports without declaring (`missing-dependency` is a lint). A
batch member counts only the modules its own package reaches by import, so one
suite's edit doesn't reach another's record. That walk is indexed once per
load: a cold `buri test //...` of the monorepo pays +0.4%. A tool's program key
adds the digests of what its last compile read that way, kept in the cache,
and compiles to learn them when they no longer hold. With none, every key is
what it was.

Instructions retired, `base` is `origin/main` at `1c674296f`, alternating:

| Workload | Before | After | |
|---|---:|---:|---:|
| monorepo, warm `build //...` | 18.05–18.07 G | 0.68–0.72 G | -96% |
| monorepo, warm `test //...` (400 cached) | 9.38–10.03 G | 1.02 G | -89% |
| monorepo, cold `build //...` | 25.65–25.73 G | 13.31–13.34 G | -48% |
| `cli/tests/example`, warm `build //...` | 230–233 M | 58–61 M | -74% |
| `cli/tests/example`, warm `test //...` | 72–73 M | 55–57 M | -23% |
| `cli/tests/example`, cold `build //...` | 668–670 M | 679.5–679.7 M | +1.5% |
| `modules 800`, one edit, rebuild `//cmd/native` | 6.89 G | 3.11 G | -55% |
| — its `link` phase | 4.17 G, 1.91 s CPU | 0.20 G, 0.04 s CPU | -95% |

The warm monorepo build went from about a second to 0.05–0.1 s. The small cold
build pays for writing the new records.

**Bookkeeping that grew with the square of the packages.** On `gen.py`'s
`pkgs_deep`, a chain of libraries, `buri lint //...` spent 1.5 G instructions
outside the compiler at 200 packages and 10.8 G at 800. Each target walked its
closure from scratch, and each lint record listed every file of its closure,
so a chain of 800 wrote 22 MB of records and read them all back to verify.

- **Closures and dependency edges are walked once per workspace.** The graph
  is the build files', and a workspace is reloaded when one moves, so
  `Workspace::closure` and `dep_edges` keep their answers. A walk that reaches
  a dependency already walked takes its closure whole. `prepare_for` walks
  the union of its targets' closures once. `check_visibility` and
  `check_cycles` borrow the shared closures and edges.
- **A lint record names its closure through its dependencies'.** The closure
  lives in a content-addressed node: the files the target's dependencies'
  nodes don't cover, with their hashes, and those nodes' keys. A node is
  checked once a run, so a chain lists and checks each file once. A node
  covers exactly the record's closure, so a record holds exactly when it did.
  Records are written after the pass, smallest closure first, so a
  dependency's node exists when its dependent's is built.
- **A library check's key folds each target once.** The fold of a target is
  its own sources and its dependencies' folds, so a chain hashes each
  library once instead of once per dependent. A target in a cycle folds its
  closure flat.

Outside the compiler's phases, instructions, one run each:

| `pkgs_deep` | 200 | 400 | 800 |
|---|---:|---:|---:|
| cold `lint //...`, before | 1.55 G | 3.88 G | 10.79 G |
| cold `lint //...`, after | 1.64 G | 3.33 G | 7.33 G |
| warm `lint //...`, before | 0.80 G | 1.83 G | 4.54 G |
| warm `lint //...`, after | 0.73 G | 1.24 G | 2.61 G |
| warm `build //...`, before | 0.48 G | 1.17 G | 3.43 G |
| warm `build //...`, after | 0.34 G | 0.80 G | 1.66 G |
| warm `test //...`, before | 0.16 G | 0.36 G | 0.86 G |
| warm `test //...`, after | 0.08 G | 0.25 G | 0.46 G |

Each doubling now costs 2.0–2.2×, where it cost 2.3–2.8×. Lint output is
identical on all 414 fixture repositories, cold, warm and one target at a
time.

**The bound counts the graph walked, not instructions.** The first bound
read the warm build's instructions outside the phases, and it was flaky: at
300 libraries one run read 306 M and the next 445 M, with no change to the
work. Most of what lands there is the kernel opening files, and that cost
moved with load: under a profiler or CPU burners nearly every run read high.
So `BURI_PROFILE` now also prints `graph work`, the targets whose edges were
read, the closures expanded and the closures folded into keys
(`workspace::graph_work`). It is 450 at 150 libraries and 900 at 300 on every
run, against 271,800 and 1,083,600 on the code before this section.
`profile::building_a_chain_of_libraries_walks_the_graph_once` and its lint
twin hold 150 against 300 to 2.3× on that count: 55 runs passed, 30 of them
beside 16 CPU burners at load 40. A closure walk keeps every target's closure
on the way back up, so each target's edges are read once.

A cold `build //...` still spends 18.6 G outside the phases at 800, and
nearly all of it is freeing analyses: each library is checked over its whole
closure. Checking them in one compilation, as `buri lint` now does, is the fix,
and it belongs to the front end.

**Measured dead ends:**

- **Serial link staging.** Ten threads hard-linking into one directory spent
  2.6 s of CPU where one spent 1.35 s, but wall time rose 20%. Three lanes cut
  the CPU in half for +5–9% wall.
- **Always-serial key reads.** Same instructions and wall, less CPU. Kept the
  threshold, since a cold page cache likely wants the threads.

**What's left:**

- A cold link of N binaries still hard-links every object once per binary.
  Naming cache entries on the command line would skip that, but `ld64`
  records each object's path in a debug stab, so the bytes would move.
- A JavaScript artifact re-emits the whole program after a one-function edit.

### 6.44 The native runtime, 2026-10-06

What a compiled program spends in `cli/runtime/`, on six release-built
programs, `main` at `526fe94b` against the ten commits below:

- **strings**: 300,000 `concat`s of `str.fromInt`, then a million of
  `str.format("item-${i}-${i * 3}")` with `split`, `contains` and `indexOf`
- **maps**: 300,000 inserts and 600,000 lookups on `"key-${i % 50000}"`
- **lists**: 3,000 rounds of `range`, `mapCtx` to a struct with a `fromInt`,
  `filter`, `reverse`, `fold` and `sortBy` over 2,000 items
- **floats**: 500,000 `str.format("${x} ${i}")` with `x` a non-integer
- **parallel**: 2,000 rounds of a 64-item `tasks.parallel` whose steps each make
  200 `fromInt`s
- **tiny tasks**: 20,000 rounds of a 64-item `tasks.parallel` of `x * 2 + i`

Instructions are the minimum of three runs. Wall is the best of five, alternating
with `main`, at load 28–47 on the ten-core M1 Pro.

| Program | Instructions before | after | Δ | Wall before | after |
|---|---:|---:|---:|---:|---:|
| strings | 4,838 M | 2,412 M | −50% | 0.31 s | 0.13 s |
| maps | 8,416 M | 7,968 M | −5.3% | 1.02 s | 0.68 s |
| lists | 7,153 M | 3,220 M | −55% | 0.34 s | 0.15 s |
| floats | 5,114 M | 787 M | −85% | 0.26 s | 0.04 s |
| parallel | 22,493 M | 11,156 M | −50% | 4.18 s | 0.36 s |
| tiny tasks | 42,053 M | 26,478 M | −37% | 4.73 s | 3.16 s |

Each change, measured against the one before:

| Change | What moved |
|---|---|
| Heap counters per thread | parallel 4.3–5.4 s to 0.42–0.53 s wall |
| Numbers render on the stack | floats −56%, lists −26%, strings −15%, parallel −37% |
| Substring search jumps to first-byte matches | strings −35% |
| An ASCII string hashes byte by byte | maps −1.9% |
| A cached size survives two idle sweeps | lists −29% |
| `str.fromInt` writes into its block | lists −14%, strings −10% |
| Floats render through Ryū | floats −64% |
| A fan-out queues one batch, one wake-up | tiny tasks −20%, 8.9–15.1 s to 3.4–5.7 s |
| A shallow task keeps its stack | tiny tasks −15% |

**Four process-wide atomics were the whole fan-out.** `buri_rt_alloc` and
`buri_rt_free` each did a relaxed `fetch_add` on `LIVE_BLOCKS`, `LIVE_BYTES`,
`TOTAL_BLOCKS` and `TOTAL_BYTES`, and with every worker allocating, those lines
bounced between cores: they were the top two Rust frames of a 64-way fan-out.
Each thread now counts into its own `Tally`, in the thread-local the block
cache already uses, and a reader sums the open tallies under a lock that an
ending thread takes to fold its counts in. So the sum stays exact, and the exit
audit sees a thread still running at exit
(`the_exit_audit_counts_threads_that_are_still_running`).

The first version cost single-threaded programs 3.5–7% in instructions: a
plain counter update is a load, an add and a store, against one `ldadd`. Two
words per operation instead of four (live is allocated less freed), one
thread-local visit for the count and the cache, and no open-check on a cache
hit brought that to within ±1%.

**macOS's allocator zeroes what it frees**, so a Rust `String` built and
dropped inside a runtime entry is a `malloc`, a `memset` and a `free`.
`fromInt`, `show` of a Float and 128-bit `show` each did that. They render on
the stack now, and `fromInt` sizes its block from the bit length and writes the
digits straight in.

**The block cache gave blocks back between two passes of the same loop.** A
sweep every 1,024 frees released any size nobody had popped since the last
sweep. Two thousand strings made in one pass and dropped in the next span two
sweeps of frees, so the second sweep handed every block to `free` just before
the next pass asked `malloc` for them again. A size now goes two sweeps
unpopped first. A drained cache still ends holding one period, because past
the grace every sweep releases.

**Ryū replaces two formatter passes per float.** `fmt.rs` took the digit count
from `core::fmt`'s shortest formatter and the digits from its exact one,
because the shortest formatter doesn't promise the closest digits.
`cli/runtime/ryu.rs` ports the reference `d2s`, which answers shortest, closest
and ties-to-even in one integer pass. Its tables are recomputed from the powers
of five in a test, the old two passes are the reference in `fmt.rs`'s tests,
and `float_parity` still matches JavaScript on 3.8 M doubles. An integral float
below 2^53 skips both and renders as its integer.

**A fan-out spent its dispatching thread in `pthread_cond_signal`.** Each step
was its own `push`: a lock, and a signal that is a system call whenever a
thread waits. The window's steps are now queued as one batch under one lock,
with one broadcast. Each finished task also re-mapped 63.75 MiB of its machine
stack to decommit it. A watermark at the top of that range, the data stack's
idea, now skips the re-map for a task that never reached it, with every
1,024th release decommitting regardless.

The tiny-tasks program is still mostly the kernel: waking ten workers per round
and the run queue's one lock. That's what's left to take.

#### 6.44.1 Round two

Against the round above, with a seventh program, `mapk`: a million `Int`
inserts into a `Map` and a million lookups.

| Program | Instructions before | after | Δ | Wall before | after |
|---|---:|---:|---:|---:|---:|
| strings | 2,417 M | 2,316 M | −4.2% | | |
| maps | 7,994 M | 6,981 M | −13% | 0.52 s | 0.46 s |
| lists | 3,223 M | 3,202 M | −0.7% | | |
| floats | 790 M | 783 M | −0.9% | | |
| parallel | 11,221 M | 10,602 M | −5.5% | | |
| tiny tasks | 29,121 M | 21,376 M | −27% | 2.82 s | 2.22 s |
| mapk | 25,538 M | 21,954 M | −14% | 1.65 s | 1.42 s |

Wall is the best of three, alternating, at load 35–40.

| Change | What moved |
|---|---|
| A fan-out waits once, through a latch | tiny tasks −20% |
| A task's body and arena slot leave their `Mutex`es | (in the row above) |
| A fan-out's step never makes its waiter list | tiny tasks −4.5%, median of eight |
| The block cache holds payloads up to 1 KiB | mapk −9.8%, maps −7.4%, others +0.4–0.6% |
| A list append checks spare slots a word at a time | mapk −4.5%, maps −5.0% |
| A free reads the heap-check mode once | strings −1.5%, lists −1.0%, maps −0.7% |

**Joining a fan-out cost more than running it.** The dispatcher joined each
step in turn, and every join went through `park_on`: a flush, a look at the
timers, and `block_on` into the reactor, even for a step already done. A
fan-out that fits the window now hands its steps one latch, which
`thread_loop` counts down where it used to wake a step's joiners, after the
step's thread is counted idle. The dispatcher waits once. Rust's `Mutex` on
macOS boxes a `pthread_mutex_t` on first use, and a task had three of them.
Two guarded fields only the running thread touches, so they're `UnsafeCell`s
now. A fan-out's step skips the third, its waiter list.

**A persistent map's node arrays outgrew the cache.** Copying a node on
insert allocates a few hundred bytes, past the 256-byte ceiling, so each copy
was a `malloc` and a zeroing `free`. The cache now keeps exact sizes up to
1 KiB. Sweeping four times the slots on every sweep cost the other programs
about 1.5%, so the new slots are swept one sweep in four.

**Measured dead ends:**

- **Spinning before sleeping.** Workers spun on the queue length for 4,096
  `spin_loop` turns before waiting, and the dispatcher on the latch. On this
  shared machine the tiny-tasks program went from 2.5–2.9 s to 5.6–6.4 s and
  doubled its instructions: spinners took the cores the woken threads needed.
- **A larger cache budget.** Ten times the per-thread share moved `mapk` 1%.

What's left in tiny tasks is the kernel waking ten workers per round and the
run queue's lock. A per-worker queue with stealing is the next shape to try.

#### 6.44.2 Round three: the pool's size

The profile said ten workers, and the pool had sixty-four. `Sched::grow`
started a thread whenever the queue was longer than the idle count, so a
64-step fan-out started a thread for every step past the idle ones, and the
pool ended at one thread per step. Each round's broadcast then woke all of
them onto ten cores.

```rust
// before
let short = self.queue.len() > self.idle && !self.starting && self.threads < MAX_THREADS;
// after
let short = !self.queue.is_empty() && self.idle == 0 && !self.starting && self.threads < MAX_THREADS;
```

An idle thread takes the next task, so the queue is short of threads only
once none is idle. `take` asks again after every pop, so a backlog behind
threads that block still grows the pool one thread at a time. Two new tests
run with a busy process on every core: steps that block on a barrier, more of
them than cores, all meet; and a fan-out whose steps hold every thread doesn't
starve the next caller's. Both hang with the pool capped.

| Program | Instructions before | after | Wall before | after |
|---|---:|---:|---:|---:|
| tiny tasks | 19,531 M | 7,454 M | 2.59 s | 1.07 s |
| parallel | 10,346 M | 8,921 M | 0.40 s | 0.24 s |

Best of three at load 46–51. The single-threaded programs don't move.

**That one line made the work-stealing rewrite unnecessary for now.** Two
smaller changes, measured on top of it, didn't pay and were dropped:

- **Waking one worker per batch, each taker waking the next** while work
  remains: tiny tasks 1.11–1.28 s became 1.26–1.52 s.
- **Taking the next task under the lock that counts a finishing thread
  idle**, which saves a lock per task: within noise on tiny tasks, and
  parallel went from 0.33–0.49 s to 0.40–0.57 s.

What's left is contention on the run queue's one lock, the largest cost in a
tiny-tasks profile after idle waiting.

**A thread-local that isn't all zeros costs every binary its full size.**
The block cache marked an unarmed thread with a sentinel in `held`, so Mach-O
stored the whole initialiser as `__thread_data`. With round two's 1 KiB slots
that was 16,552 bytes, and a stripped hello world on CI's macOS went 2.4 KB
past its 512 KiB ceiling. The cache now keeps a `limit` that is zero until it
is armed and an `armed` flag, so the thread-local lands in `__thread_bss`:
`__thread_data` 56 bytes, stripped hello 477,600 to 461,088 bytes locally.

#### 6.44.3 Round four: the allocator's fixed cost

With the formatting, search and scheduling costs gone, `buri_rt_alloc` and
`buri_rt_free` were the largest runtime frames in every single-threaded
profile. Each change below shaves instructions every allocation or free
pays:

| Change | strings | lists | floats | maps |
|---|---:|---:|---:|---:|
| A free's cache push inlines, the rest out of line | −5.1% | −4.2% | −4.3% | −2.4% |
| One byte compare for the heap-check mode | −2.2% | −1.8% | −1.8% | −1.0% |
| A free skips the scalar-index keys until an index exists | −4.2% | −2.4% | −3.6% | −1.2% |
| A split takes its pieces' references in one update | −1.6% | | | |

| Program | Instructions before | after | Δ |
|---|---:|---:|---:|
| strings | 2,286 M | 1,998 M | −13% |
| lists | 3,170 M | 2,905 M | −8.4% |
| floats | 775 M | 703 M | −9.4% |
| maps | 1,717 M | 1,636 M | −4.7% |
| mapk | 2,930 M | 2,921 M | −0.3% |

- **The push closure was its own function.** The closure `cache_push_counted`
  runs inside the thread-local carried the arm, the refusal, the sweep tick
  and the large-block path, so it compiled to a second frame on every free.
  Its fast path is now an accepted push between two sweeps.
- **The heap-check mode was decoded on every allocation and free**, about
  eight instructions each. Both now compare the cached byte with `Off`'s.
- **`scalars::forget` read four atomic keys on every free**, for an index
  only a long non-ASCII string ever builds. A flag, set before the first key,
  makes it one load until then.

Tiny tasks: sharing one lineage across a fan-out's steps saves a `Vec` and a
`thread::current()` per step, about 4% of the median instructions. The rest
of that program is the run queue's lock and the task-stack pool's lock, which
every finishing step takes; per-worker queues would remove only the first,
and neither moves wall time much while wake-up latency dominates a round.

### 6.45 What `--release` hands to LLVM, 2026-10-06

`buri build --release` of `mixed-10k` spent 20.2 G instructions in `emit`:
0.6 G building IR, 8.4 G in `opt` and 11.1 G generating code. After `opt` the
unit held 1,521 functions, and 880 of them had fewer than five instructions.
`llc` spends about 1.1 M instructions on a function before it looks at the
body, and `opt` about 0.8 M, so those 880 cost more than all the code in them.
Most were external functions inlined at every call. LLVM kept them because they
were external, and the linker stripped them afterwards.

Four changes, each its own commit:

- **An object leaves out functions nothing will call** (`backend/linkage.rs`,
  `Unit::prune`). A unit doesn't emit a function nothing names. Once `opt` is
  done, it deletes one no other unit names that nothing in the unit still
  uses. `opt` still sees every survivor as external, so its inlining doesn't
  change. Internal linkage for the same functions saved 17%, but then LLVM
  inlined `shapes`'s whole loop into `main`, and the program ran 12% more
  instructions.
- **A unit's key says which of its functions another unit names.**
  `actions::unit_hashes` adds a line per such function, so a cached object
  that dropped a function doesn't serve a program in which another unit calls
  it. Without that line,
  `native::llvm::a_call_into_a_cached_unit_links_when_it_is_added_and_when_it_is_removed`
  fails with `undefined symbol`.
- **An object's unit discards value names**, as `clang` does outside a debug
  build. `emit_ir_text` keeps them, along with every function, for the tests
  that read IR.
- **A developer build verifies our IR once.** `verify_each` checked the output
  of every `default<O2>` pass. That checks LLVM's work, not ours, and it made
  `opt` seven times the work. The single check before the pipeline stays, and
  `a_developer_build_rejects_a_malformed_module` holds it.

`buri build --release`, `emit` phase, two alternating runs each:

| Program | Before | After |
|---|---:|---:|
| `mixed-10k` | 20.25 G | 17.72 G (-12.5%) |
| `mixed-1k` | 2.42 G | 2.10 G (-13.2%) |
| `derive-heavy-1k` | 2.32 G | 2.01 G (-13.3%) |
| `few-large-fns-1k` | 74.0 M | 67.5 M (-8.8%) |
| `wide-match-1k` | 327 M | 120 M (-63%) |
| `many-small-fns-1k` | 536 M | 92 M (-83%) |
| 125 programs: every matches batch, every fourth growth case, §6.32's shapes at 50 and 100 | 234.1 G | 215.0 G (-8.2%) |
| `mixed-10k` with a dev-profile toolchain | 68.6 G | 20.4 G (-70%) |

**Run time doesn't move.** All seven run-time programs link to the same bytes
before and after: `fib(32)`, a tree of 2¹⁶ nodes built and walked 20 times,
lists of 100k pushed, folded, mapped and sorted, 400k templates, a 20 M-step
enum state machine, and 20 M payload enums with and without `Str`. Of the 124
programs in the set above that link, 118 link to the same bytes. The other six
run the same instructions: their data sits 16 bytes lower, because a constant
only a dropped function used is gone, or only their UUID differs.

**Tried and dropped:**

- **Payloads wider than 16 bytes held as words** (`repr::WIDEST_INT_BLOB` at
  16 rather than 64). It saved 1.4% on `mixed-10k` and 2.4% on the 125
  programs, but the lists program ran 0.27% more instructions.
- **Skipping the count on a constant null.** `opt` folds those branches
  almost for free. It saved 0.4%, and 12 objects changed.
- **Ordering units by IR size** instead of member count. A simulated schedule
  of `mixed-10k`'s units finished no sooner.
- **`-O3`**, measured and not changed. O3's IR pipeline made the lists program
  10% slower. O3's code generation saved 1–3% of instructions on three
  programs and no wall time.

**What's left is LLVM's fixed cost.** Running `opt` on IR it has already
optimized still costs 83% of the first run, so cleaner IR could save at most
17% of `opt`. A unit costs about 13 M before its first function, mostly to
build the code-generation pass pipeline, which the C API builds again for
every module.

**Run time: an increment and a decrement that cancel.** Profiles of the seven
run-time programs, the runtime's twelve and the map kernel spend most of their
generated code's time in one shape. A match binds a field and then drops the
value it matched on, so the middle end plans:

```text
v23 = payload.#2.1 v4     ; a map node's children
incref v23
decref v4                 ; the node, whose only count is v23
```

That pair does nothing, but LLVM can't fold it. After inlining it's an
increment of a count followed by a decrement of the same one, and LLVM can't
tell the decrement doesn't reach zero and free the block. A `!range` on the
count load, and testing for the last reference before the immortal one, left
the binaries byte-identical. `Unit::cancelled_counts` leaves out an `incref x`
and the `decref w` after it when only instructions that can't call, allocate
or count lie between them, and `w` is `x` or holds no count but `x`'s. A field
behind a box doesn't qualify, and neither does a variant with a second counted
field. `e2e::a_field_taken_out_of_what_a_match_drops_keeps_its_count` holds
those cases, with every value also held in a list or a second binding while
its string grows. It leaks four blocks if every field is allowed to cancel.

Best of nine alternating runs:

| Program | Instructions before | After | Wall before | After |
|---|---:|---:|---:|---:|
| `pairs`, 20 M `Str` payloads | 1,068.3 M | 895.0 M (-16.2%) | 68.7 ms | 67.1 ms |
| `pmaps`, a `Map<Str, Int>` | 7,385.3 M | 6,529.5 M (-11.6%) | 443.0 ms | 429.2 ms |
| `mapk`, 1 M inserts and gets | 2,926.4 M | 2,682.8 M (-8.3%) | 234.1 ms | 219.4 ms |
| `maps` | 1,716.5 M | 1,594.3 M (-7.1%) | 111.3 ms | 105.6 ms |

The other fifteen programs are within noise. `emit` for the nineteen fell from
5.25 G to 4.55 G instructions, `mixed-10k`'s is flat, and the nineteen
executables shrank by 272 bytes.

**Tried and dropped for run time:**

- **`noalias`, `nonnull` and `align 16` on `buri_rt_alloc`'s result.** Every
  program stayed within 0.12% of its old count, which is noise: the same
  binary reads ±0.25% from one run to the next.
- **A branching increment** (`rc == IMMORTAL` or `rc + 1`), so that an exact
  `rc + 1` would meet the decrement. LLVM folded no pair, and `pairs` ran 1.9%
  more instructions.

What's left in the profiles is mostly the runtime's: `buri_rt_alloc`,
`buri_rt_free`, the thread cache, `write_decimal` and the map's node copies.
The hot generated loops are the merge in `sortBy` and the pointer chase in
`core_map.find`, and both already compile tight.

### 6.46 The lexer's constant and the shared prelude, 2026-10-06

A second front-end round after §6.41. These are instructions per repetition,
`main` at `526fe94b` against the four commits below. `check` is the `sema`
child minus the `load` child.

| Corpus | `lex` | `lex+parse` | `check` |
|---|---:|---:|---:|
| `mixed-100k` | 89.4 → 76.5 M (−14%) | 196.9 → 181.8 M (−7.7%) | 451 → 441 M (−2.2%) |
| `comment-heavy-100k` | 107.0 → 69.8 M (−35%) | 162.4 → 126.9 M (−22%) | 253 → 246 M |
| `mixed-many-files-100k` | 102.4 → 91.3 M (−11%) | 225.7 → 211.3 M (−6.4%) | 405 → 379 M (−6.5%) |
| `match-heavy-100k` | 83.6 → 73.8 M (−12%) | 179.4 → 166.7 M (−7.1%) | 437 → 432 M |

Each lexer change, measured against the one before it:

| Change | `mixed-100k` lex | `comment-heavy-100k` lex |
|---|---:|---:|
| The one space after a token is stepped over before the `match` | −6.6% | |
| A `//` comment's end is found with `str::find`, which is `memchr` | −5.3% | −12.8% |
| A `Comment` holds where its text is, not a `String` copy | −1.3% | −22% |

A file of one-letter words separated by spaces made the first one plain: a
word cost 126 instructions, and the space after it about 29 of them, all
spent on a trip through the jump table and the blank-run loop. It's 97 now.

§6.5's dead end for comments measured lex+parse *time* on `mixed`, where a
word-at-a-time scan is under the noise. Measured in instructions, and on
comments, it's the largest single lexer change here.

**Every module shares one prelude table.** `resolve_scopes` built each
module's `names` by copying its own declarations, then every prelude name, a
`String` each. `ModuleScope` now keeps only its named imports:

```rust
scope.name("Option")      // imports, then `own`, then the shared prelude
scope.visible()           // each visible name once, in no particular order
```

The prelude is one `Arc<Prelude>` per compilation. The language server's nine
reads moved to `name` and `visible`. `mixed-many-files-100k` gains most,
because it has the most modules per line.

Formatted output and token shapes are identical on all 7,806 checked-in
`.buri` files and six pinned corpora. The full workspace suite and the LLVM
`native` suite pass.

**Measured and dropped:** skipping the final resolution walk in a body that
made no type variable read within noise. Nearly every body makes one, for a
literal.

**What's left.** Identifiers now cost about 70 instructions each, and the
parser about 110 per token, spread across the descent with no single hot spot.
Dropping a `Checked` is still about a tenth of a `sema` repetition: one `free`
per `Box<Expr>` and `Vec` of the typed tree, which `middle` reads and
rewrites.

### 6.47 `buri lint` and the language server, 2026-10-06

`buri lint //...`, cold, process instructions, repositories from the algorithm
audit's `gen.py`. `main` is `dc96906ac`, after §6.43 made `resolve_module` a
lookup; the audit's numbers, before that, are in brackets:

| Repository | `main` | After | Δ |
|---|---:|---:|---:|
| `pkgs_deep` 200, a chain of libraries | 10.47 G (24.76 G) | 2.03 G | −81% |
| `pkgs_deep` 50 | 1.05 G | 0.48 G | −55% |
| `pkgs_layered` 40, 6 layers of 40 | 4.11 G (8.48 G) | 2.00 G | −51% |
| `pkgs_layered` 10 | 1.05 G | 0.56 G | −46% |
| `pkgs_wide` 100 | 1.00 G | 0.83 G | −17% |
| `big_file` 1000, 32k lines | 1.84 G | 0.43 G | −76% |
| `modules` 100 | 0.50 G | 0.33 G | −33% |
| `mixed-10k` | 0.36 G | 0.24 G | −33% |
| `mixed-10k` × 10 binaries | 3.01 G | 1.86 G | −38% |

The audit measured a whole JavaScript build of the chain at 2.4 G, and of the layers at 1.6 G.

**Every target is checked in one compilation.** Lint analysed each target on
its own, so a library was loaded and checked again for every target that
reaches it. The targets the lint records can't answer are now loaded and
checked together (`driver::load_programs`), and each target's rules read its
share of that compilation: the modules its own load would hold, and the
diagnostics in their files. The share declines, and the target is analysed
alone as before, wherever its own answer could differ:

- loading said something other than a parse error, which can depend on the
  order modules were reached in (`circular-import`), or is said once per
  compilation (a generator's diagnostics);
- a module was loaded in a role an importer would decide differently;
- a diagnostic lands outside every module's file.

**Several binaries share it too.** The checker kept one function per
entry-point name for the whole compilation, so a second binary's `main` was
refused its `context`, and every binary after the first was analysed alone.
Entry points are now kept per entry module (`Checker::module_entries`,
`entry_points`), and `unknown-entry-function` is asked of each binary's
`main.buri` against its own exports. `buri lint //...`, cold, process
instructions:

| Repository | Before | After | `check` before | after |
|---|---:|---:|---:|---:|
| 10 binaries sharing a library of 2,000 functions | 0.70 G | 0.26 G (−64%) | 410 M | 53 M |
| `pkgs_layered` 40 | 2.03 G | 1.94 G (−5%) | 118 M | 64 M |
| `pkgs_deep` 200 | 2.12 G | 2.06 G (−3%) | 99 M | 54 M |

`gen.py`'s repositories have two binaries, so checking halves there, but most
of their lint is outside the compiler (below). Output is identical on all 414
fixture repositories, cold, warm and one package at a time.
`build::incrementality`'s
`each_binary_in_one_lint_builds_its_context_in_its_own_main` lints two
binaries in one run and expects only the `context` built outside `main` to be
refused. With the sharing and without the per-module table, it also refused
the first binary's `main`.

Checking one module can't depend on what else is loaded: a name resolves
through the module's imports, and a method through its receiver's type, whose
`impl`s live in the type's own module.

**The rules are linear in a file.** Five rules lexed every module for
themselves; they share one lex. The type census asked every declaration about
every identifier, and binary-searches now. `todo-comment` searched every gap
between tokens three times, and skips a gap with no marker letter in it.
`compiled_by` scanned the package's sources once per function, and runs once
per module. The rules read the package's modules and bodies from an index
instead of walking the compilation once per rule, and `unused-type` reads
what `dead-code` said as the report grows rather than rereading it per target.

**A keystroke relints one file's text.** The rules that read only a module's
text keep their findings, and its identifiers, under the text's length and
hash, the test `build::sources` already trusts. Each edit lexes the edited
file and nothing else. The workspace sweep checks its stale targets through
the same shared compilation, and `convert::position_of` counts lines as bytes
instead of decoding the file from the top for every finding.

Per keystroke, a `textDocument/didChange` and a pull, from `lsp.py`:

| Repository | `main` | After | Δ |
|---|---:|---:|---:|
| `big_file` 250, 8k lines | 226 M | 83 M | −63% |
| `big_file` 1000, 32k lines | 1,692 M | 320 M | −81% |
| `big_file` 4000, 128k lines | 20,941 M | 1,275 M | −94% |
| `modules` 400, one package | 1,174 M | 621 M | −47% |

Each now grows linearly. `build::profile`'s `linting_checks_each_library_once`
and `the_lint_rules_are_linear_in_a_files_length` guard the two lint shapes;
on `main` their phases grow 3.3 and 2.7 times per doubling.

Output is identical to `main`'s on all 414 fixture repositories under `cli/tests`: the
cold run, the warm run from the records, and one package at a time.

**Kept as it was.** `Sources` reads and hashes every file it holds on every
question, and that's most of a keystroke in a package of 400 files. Skipping a
file whose modification time and size held still would miss a rewrite that
restored both, as `rsync -t` does. A key on the change time as well, trusted
only once that time is older than the read, as Git does for its index, would
be safe short of a clock stepping backwards.

**A cold `buri build` checks its libraries the same way.** It checked each
library over its whole closure, so a chain checked its first library once per
library after it, and freed each of those analyses outside the compiler's
phases. The libraries and tools a build checks now go through one shared
compilation (`driver::shared`, which lint's `Shared` now wraps too), with
every body checked as `driver::analyze` checks them. Each library prints its
own share of the diagnostics, in the order its own check sorted them, and
records its own reads for its clean-check record. Anywhere the share could
differ, it declines the same way, and each library is checked alone as
before. Cold `buri build //...`, process instructions:

| Repository | Before | After | `lex+parse` | `check` | other |
|---|---:|---:|---:|---:|---:|
| `pkgs_deep` 800 | 138.1 G | 13.7 G (−90%) | 59.0 → 1.1 G | 54.8 → 0.5 G | 18.6 → 6.3 G |
| `pkgs_deep` 200 | 10.8 G | 3.3 G (−69%) | 3.57 → 0.27 G | 3.59 → 0.15 G | 2.07 → 1.28 G |
| `pkgs_layered` 40 | 5.21 G | 3.75 G (−28%) | 1.10 → 0.37 G | 0.82 → 0.18 G | 1.41 → 1.38 G |

On the fixture repositories, every build prints the same thing, cold and warm,
and writes the same objects, JavaScript outputs and cache records, with key
hashes masked because keys hold the toolchain's identity.
`build::profile`'s `a_cold_build_of_a_chain_of_libraries_checks_each_once`
guards it: on `main` its loading and checking grew 3.6 times per doubling at
100 libraries, and 1.4 times after. The teardown lands in "other", which
also holds the bookkeeping the warm build's bound already measures.

The bound counts modules loaded (`modules loaded` in the profile) and the
`check` phase's instructions, not loading's. Loading's instructions are
mostly the kernel reading files, and beside CPU burners they read 77–120 M
for 200 libraries from run to run, so the bound read anywhere from 1.36× to
1.93×. The count is 100 and 200 on every run, 5,050 and 20,100 on the code
before this section. 30 runs at load 33–57 read 2.00× loading and
1.06–1.08× checking.

**What's left.**

- **Lint's own bookkeeping is `n²` in a chain of packages.** On `pkgs_deep`,
  "other" is 1.76 G at 200 packages and 11.7 G at 800. A profile at 800 puts
  it in `Workspace::closure` once per target, `Cache::put` writing each
  target's record with its whole closure, `check_cycles`, `check_visibility`
  and `Workspace::dep_edges`. All of it is in `cli/src/build`.
- A keystroke still checks and lints the whole target. Reusing the other
  modules' bodies needs the checker to say when an edit left every signature
  where it was.

### 6.48 `core/map` updates in place, 2026-10-06

A million inserts and a million gets into a `Map<Int, Int>` took 25 G
instructions in `--release`, 25 thousand an insert. Half of a debug run was
`glue$retain` and `glue$elems`. Two things made every insert copy its whole
path:

- **Every level was rebuilt by four list operations.**
  `xs.take(at).push(x).concat(xs.drop(at + 1))` retained each child into the
  copy, and the old list released each one when it died.
- **Nothing below the root was ever unique.** `children.get(idx)` hands back a
  second count on the child, and the option holding it let go only after the
  descent, so the next level down found its list at two.

`insertAt`, `replaceAt` and `removeAt` are runtime splices now
(`cli/runtime/splice.rs`), and they **own** the list:

```text
rc.rs   TAKEN_NATIVELY = [("map.insertAt", 1), ("map.replaceAt", 1), ("map.removeAt", 1)]
```

A caller that keeps the list takes a second count first, so a count of one
means nobody else can see the block, and the splice writes into it. An append
can be in place with a borrowed list, because it writes past every alias's
end. A splice writes inside one. A shared list is copied, and the splice gives
its count back. JavaScript copies, so it has nothing to mark.

`insertNode` takes the child out of its list before descending, and binds the
lookup to a name:

```buri
let found = children.get(idx);       // let go of where the match takes it apart
match (found) {
    .Some(child) => {
        let rest = removeAt(ctx, children, idx);   // the list lets go of the child
        let (grown, added) = insertNode(ctx, child, hash, key, value, depth + 1);
        (.Branch(bitmap, insertAt(ctx, rest, idx, grown)), added)
    },
    …
}
```

So each level holds its child once, and the whole path updates in place.
`removeNode` does the same. `remove` asks `has` first. Before, a miss answered
`self`, which kept the old root alive beside the descent.

| Kernel, instructions retired | `main` | After |
|---|---:|---:|
| 1M inserts + 1M gets, debug | 37.4 G | 5.35 G |
| 1M inserts + 1M gets, `--release` | 25.5 G | 3.13 G |
| 300k inserts, 150k removes + 150k misses, debug | 15.2 G | 2.27 G |
| the same, `--release` | 10.5 G | 1.37 G |

Wall time for the first kernel, two warm runs at load 22–30: debug 7.1–9.0 s
before, 1.9–2.1 s after; `--release` 5.3–8.0 s before, 0.54–0.55 s after. A
fresh executable's first run waits in §6.39's system check, so time the second.

`agreement`'s `a_map_updated_through_one_name_is_unchanged_through_another`
and `a_map_shared_with_a_task_is_unchanged_by_either_side_s_update` run on
JavaScript, stencil and LLVM under the heap check. They hold a map by a second
name, in a list of versions, and across a task, then update one name and read
the others. `splice::tests` covers the runtime's in-place, copy, grow and
emptying paths.

**Tried and dropped:**

- **Releasing a temporary scrutinee at an arm's entry.** `rc` would give
  `match (xs.get(i))` the dying-local treatment `insertNode`'s named lookup
  gets, which subsumes that workaround. Nothing else gained: a heap, an
  ordered map and the matches corpus moved under 1%. Run beside the old pass
  over 498 programs, it added 105 retain-and-release pairs, where a read the
  pre-pass called consuming was a borrow to the scan, and changed 13 functions
  in ways a per-name diff couldn't account for. It's parked on the branch
  `rc-temp-scrutinee`, with the two `native::ownership` rows that pin it.
- **Building `rc::Syntactic` once per native build** instead of three times:
  about 0.8% of the middle end on `mixed-100k`.

### 6.50 A higher opt-level for the tests' `buri`, measured and left, 2026-10-06

The suite's `buri` is built at `opt-level = 1`. Four other profiles were
built from `dc96906ac`, each in a target directory of its own, by passing
`--config` rather than editing `Cargo.toml`:

```sh
cargo nextest run --workspace --exclude website --exclude buri-llvm \
  --config 'profile.dev.package.buri-middle.opt-level=2'   # one per crate
```

| Profile | Change from today's |
|---|---|
| all at 2 | the 13 toolchain crates at `opt-level = 2` |
| hot at 2 | `buri-syntax`, `-semantics`, `-middle`, `-stencil` and `-js` at 2, the rest at 1 |
| all at 3 | the 13 at 3 |
| no line tables | `profile.test.debug = false` |

`debug-assertions` stayed on in all of them. Every run was alternated with the
others at load 17–63, and CPU is user plus system:

| Profile | Cold build, CPU-s | Suite run, CPU-s | `buri test //...` on `cli/tests/example` |
|---|---:|---:|---:|
| today | 648, 682 | 589–663 | 1,196 M instructions |
| all at 2 | 748 | 645, 680 | 1,102 M |
| hot at 2 | 647, 666 | 648, 670 | 1,118 M |
| all at 3 | 791 | 631, 666 | 1,068 M |
| no line tables | 561–611 | 570, 659 | 1,200 M |

- **`buri` gets 7–11% faster and the suite doesn't notice.** `buri` is about
  half the suite's CPU (§6.21), and its own instructions are only part of
  that. 8% of that is about 25 CPU-seconds, and two identical runs here
  differ by up to 75.
- **The build does notice.** A package override reaches every target in the
  package, so "all at 2" also builds `buri`'s 15 test binaries at 2. They're
  most of a cold build: `cargo build --bin buri` alone is 208 CPU-seconds.
  "All at 2" adds 80–140 CPU-seconds to a cold build and "all at 3" adds
  125–170. "Hot at 2" adds almost nothing and saves almost nothing.
- **Incremental builds don't move.** After touching
  `crates/middle/src/lib.rs` or adding a function to it, every profile
  rebuilt in 13–41 s and 19–25 CPU-seconds.
- **Neither does a cold build with sccache warm.** It took 82–144 s for every
  profile, in no consistent order. The sccache server compiles misses outside
  cargo's process tree, so this row has no CPU figure.
- **Wall time isn't quotable.** The suite took 207–668 s. The first run after
  a fresh build was usually the slowest, because its new executables queue for
  the system-wide check (§6.39).
- **`266070021` showed the same.** Over two alternating rounds, the suite took
  665–724 CPU-seconds at all four opt-levels. Cold builds took 608–646 today,
  762–778 for "all at 2", 639 for "hot at 2" and 789–814 for "all at 3".

**Line tables stay.** On macOS the debug information stays in the object
files, so `debug` only changes the debug map in `__LINKEDIT`: 8.5 MB → 4.5 MB of
`buri`'s 66.6 MB. `__TEXT` is 57.9 MB either way, and the dev profile's
full-debug `buri` is also 66.6 MB. The `native` test binary goes from
77.0 MB to 73.6 MB. Six alternating rounds timed the first exec of a fresh
copy at load 25–48:

| Binary | First exec, median | Range |
|---|---:|---:|
| `buri`, today | 4.7 s | 1.9–11.1 s |
| `buri`, no line tables | 3.5 s | 1.0–11.1 s |
| `buri`, all at 2 | 3.8 s | 1.0–10.1 s |
| `native`, today | 3.8 s | 1.1–8.3 s |
| `native`, no line tables | 3.8 s | 2.4–8.5 s |

A second exec took 4–15 ms. The wait is the queue, not the size (§6.39). Going
without line tables saves about 60 CPU-seconds per cold build and nothing in
the suite, and every backtrace loses its line numbers.

**A faster `buri` exposed a race.**
`fuzz::the_watchdog_reports_a_toolchain_that_does_not_stop` failed once at
"all at 2". `hang::launched` polls every 10 ms, and a no-op build that exits
before the first poll beats a zero cap. "All at 3" failed one test once, but
its log was overwritten before it was read.

### 6.51 The suites wait on the check, not the CPU, 2026-10-06

Both suites spend most of their wall time in §6.39's check of each new
executable, which runs one file at a time for the whole machine. The CPU they
use barely moves. Here's the LLVM `native` suite, built once at `d0c9b730a`,
at load 45:

| Programs | Tests | CPU, user + sys |
|---|---:|---:|
| all freshly linked | 616 s | 230 s |
| all checked by an earlier run | 64 s, 55 s | 242 s, 249 s |

`kept::settle` already runs a program from the first file that held its bytes.
Any compiler change gives every program new bytes, though, so a run after one
pays a check for each of its ~580 programs. The workspace suite pays them on
every run. Each `buri test` in a scratch repository links a new runner, and
`repositories::snapshots` alone runs 65 cases of them.

A check takes 0.2 s on a quiet machine. With other agents' suites in the same
queue it took 2.5–5 s, and 6 s for a 70 MB test binary. That's how one test
runs 0.3 s alone and 33 s in a full run.

**Three changes landed.**

- **Generated matches run the middle end once.** `check` emitted each
  program twice per native backend, once in `native_refusal` and again in
  `run_native`. It also ran the middle end once per backend. Now one
  `prepared_native` program goes to every backend, and `emitted` runs once
  per backend, which answers both "does it refuse?" and "what does it
  print?". The agreement rows share the prepared program the same way.
- **The native set runs each file's front end once.** Each shard ran
  `missing_for`, which is the front and middle end, before `linked` ran both
  again. `linked`'s build already asks the same question.
- **`test-threads = 36`** in `.config/nextest.toml`, three times this
  machine's cores. A test waiting on the check holds a nextest slot while the
  CPU idles.

Instructions, `cargo test` on the native test binary, 4 threads, two runs
each, both identical:

| Tests | Before | After |
|---|---:|---:|
| `matches::` | 110.3 G | 56.4 G (−49%) |
| `conformance::the_native_set_passes` | 25.1 G | 20.1 G (−20%) |
| `agreement::` | 36.2 G | 35.6 G (−1.7%) |

Workspace suite, test phase, alternating, load 25–70:

| `test-threads` | Runs | Median |
|---|---|---:|
| 12 | 312 s, 179 s, 141 s, 162 s, 199 s, 138 s, 182 s | 179 s |
| 24 | 258 s, 169 s, 169 s, 189 s | 179 s |
| 36 | 124 s, 142 s, 181 s, 101 s, 134 s, 108 s, 156 s, 118 s, 129 s, 125 s | 127 s |

The first 12-thread run also checked freshly built test binaries. The 36
column includes the two experiments below, which changed nothing measurable.

LLVM `native` suite, every program freshly linked, alternating:

| `test-threads` | Tests | CPU, user + sys | Load before → after |
|---|---:|---:|---|
| 12 | 391 s | 229 s | 43 → 32 |
| 36 | 192 s | 196 s | 32 → 6 |
| 12 | 187 s | 207 s | 6 → 9 |
| 36 | 89 s | 228 s | 9 → 12 |

**Measured and dropped:**

- **`repositories::snapshots` in eight shards.** It's the last test running in
  most runs. In shards, a shard was last instead, and the totals didn't move:
  138 s and 182 s at 12 threads, 156 s and 118 s at 36.
- **A pool twice the cores wide** (`harness/pool.rs`). At 36 threads: 134 s and
  108 s against 181 s and 101 s.

**What's left** is fewer new executables per run, which the harness can't
provide without changing what a test checks. `buri` could run a linked
program from a store keyed by its bytes, as `kept::settle` does. Then a rerun
with an unchanged compiler would check nothing. Rerunning to rule out a flake
is that case. Smaller test binaries would make each check of a test binary
shorter: `debug = "line-tables-only"` is with the `Cargo.toml` profile work.
Adding the terminal to Developer Tools would skip the check entirely, but
that's ruled out on this machine.

### 6.52 An arena for the typed tree, measured and left, 2026-10-06

The proposal: each `typed::Body` holds its nodes in a `Vec<Expr>`, `Box<Expr>`
becomes an `ExprId` and `Vec<Expr>` an `ExprList`, so a body allocates a few
vectors and frees them in one go. It would touch about 1,100 `ExprKind`
references across `semantics`, `middle`, `js` and `cli/src`, so the prize was
measured first.

On `mixed-100k` the measuring harness swapped the global allocator three ways:
the system's; the research prototype, a size-class front end with per-thread
free lists (`fast`); and a bump allocator that never frees (`bump`), which
bounds what allocating and freeing cost at all. Instructions retired, best of
two or three:

| | system | fast | bump |
|---|---:|---:|---:|
| check (`Checker::run`) | 368 M | 287 M | 307 M |
| dropping the `Checked` | 64 M | 15 M | 9 M |
| middle end, every pass | 1,518 M | 1,199 M | 1,024 M |
| … dropping the typed tree | 59 M | 12 M | 8 M |
| … `monomorphize` | 171 M | 108 M | 123 M |
| … `inline` | 196 M | 135 M | 152 M |
| … `rc::analyze` | 418 M | 265 M | 303 M |
| … `lower` | 394 M | 530 M | 290 M |

- **On the system allocator the arena would pay.** Allocating and freeing the
  typed tree is about 220 M of the middle end and 117 M of check plus the
  drop, around 18% of the two together.
- **After a fast allocator it wouldn't.** What an arena still saves is the
  cheaper drops and per-node allocations that now cost little, an estimated
  3–4%, under the 5% it had to clear.
- **The allocator is the larger and cheaper change.** It took a fifth off check
  and the middle end, IR and `rc` included, which an arena never touches.
- **Except `lower`, 35% slower on the prototype.** `lower` runs across the
  cores, and its results are freed on other threads, which the prototype hands
  to the freeing thread's lists. That cross-thread traffic is the likely cause.

**Check wall time before building the allocator.** The research agent found
mimalloc cut instructions by 28–36%, but wall time barely moved: about 8% on
sema and flat on parallel lowering. The in-tree prototype made parallel
lowering two to three times slower in wall time. Counts can't see contention
or cache behaviour (§8, "What counts can't see"), so the allocator needs a
wall-time comparison on a quiet machine first. §6.54 has it.

### 6.53 The JavaScript runtime's helpers, 2026-10-06

Native had a day of run-time work and `--output=js` had none. Twenty programs
ran on both engines, built `--release` for `node`: the native run-time apps
(strings, maps, lists, floats, tasks, their parallel forms), `mkrt.py`'s seven,
and the research `mapk` kernel. `bun --cpu-prof` and `node --cpu-prof` wrote
the profiles.

**One helper was 70% of a map program.** `core/map` tests a slot with
`popCountU64`, and the runtime counted one `BigInt` bit at a time:

```js
while (v) { n += v & 1n; v >>= 1n; }   // a BigInt op, and an allocation, per bit
```

The counts read the word as two 32-bit `number`s now, and the three counts and
every shift count read their answer from a table of `0n` to `1023n` rather
than calling `BigInt()`. Lengths and indices read from it too.

Two smaller ones:

- **`sortBy` sorted index pairs.** `Array.prototype.sort` is stable, and
  `Order`'s tags are 0, 1 and 2, so `order(a, b) - 1` is the comparator. The
  built-in sort moves `undefined` to the end without asking, and `None` is
  `undefined`, so a list holding one still sorts pairs.
  `lists.buri`'s `sortBy orders None by the comparator` pins that.
- **`range` counted in a `BigInt`.** The loop counts in a `number` now.

Best of three, alternating, at load 7–27. CPU is user plus system; the counts
are instructions retired, which moved least with the load:

| Program | bun CPU before | after | instructions | node CPU before | after | instructions |
|---|---:|---:|---:|---:|---:|---:|
| `mapk`, 1M inserts + 1M gets | 14.5 s | 4.9 s | −74% | 6.4 s | 2.8 s | −66% |
| `kern/mapk` | 14.6 s | 4.9 s | −74% | 6.7 s | 3.1 s | −67% |
| `maps`, `Str` keys | 5.4 s | 1.6 s | −74% | 2.5 s | 0.95 s | −69% |
| `pmaps` | 38.1 s | 9.7 s | −79% | 10.6 s | 3.0 s | −79% |
| `rt_lists` | 0.91 s | 0.73 s | −6% | 0.41 s | 0.31 s | −20% |
| `lists` | 1.66 s | 1.53 s | −9% | 0.64 s | 0.54 s | −13% |
| `plists` | 12.8 s | 11.8 s | −8% | 4.0 s | 3.6 s | −9% |
| `tasks` | 2.62 s | 2.44 s | −7% | 0.84 s | 0.87 s | +5% |
| `seqwork` | 2.43 s | 2.25 s | −6% | 0.84 s | 0.86 s | +4% |
| `rt_pairs` | 1.64 s | 1.56 s | −5% | 0.46 s | 0.48 s | +4% |
| `fib`, `tree`, `machine`, `shapes` | | | ±1% | | | ±1% |

The other six took 4–8% fewer instructions on bun and about 1% more on node.
Every output matched.

**The table costs node what it saves bun.** Reading a length from it is 6–8%
fewer instructions on bun and 3–4% more on node, in the programs that ask
`length()` in their loop. Bun's figures are two to three times node's, so it
stays.

**What's left is `BigInt`.** `Int` is a `BigInt` on this backend
([`resolved-questions.md`](./resolved-questions.md)). A loop step of an add, a
remainder, an increment and a compare takes 44 ns on bun and 14 ns on node,
against 3 ns on `number`s. The emitted code for `fib`, `tree` and `machine` is
already a loop of `BigInt` operations and array reads, with no runtime helper
left in their profiles. That's also why node runs them two to three times
faster than bun.

Code size: `cli/tests/example`'s two JavaScript artifacts went from 70,821 to
70,923 bytes, and the golden corpus from 332,653 to 333,735, with the generated
half unchanged. That's the table and its reader, about 100 bytes in a program
that asks a length.

**Tried and dropped:**

- **Splicing a map node's children in place.** §6.48's idea applies: `$u`
  marks a list nothing else holds. But a branch's children are `$share`d where
  the match binds them, so only `insertAt` over `removeAt`'s fresh list could
  write in place. Done by hand in `mapk`, that saved 7% of CPU. It needs the
  JavaScript sharing plan to hand `map.insertAt` its receiver
  (`rc::TAKEN_NATIVELY`, `crates/middle`), so it's left.
- **Every sharing mark off**, unsound and only a ceiling: 5–8% on `lists`,
  `plists`, `maps` and `mapk`. The marks aren't where the time goes.
- **Skipping `$shareEach` over a list of primitives:** under 1%.
- **Copying the map splices with `push` instead of `slice` and `splice`:**
  faster in a microbenchmark, 3–10% more instructions in `mapk` and `maps`.
- **A preallocated `new Array(n)` in `range`:** V8 marks it holey, and
  `tasks.parallel`'s `map` over one made `tiny` twice as slow on node.
- **Dropping `chunkAt`'s `asIntN` around `& 31n`:** ±2%.
- **Objects instead of arrays for variants,** measured on a microbenchmark
  only: 8% slower on bun and 26% faster on node. It'd change the whole value
  representation, so it wasn't tried.
- **`BigInt` to string through `Number`,** and **a scan instead of the
  surrogate regex:** slower on bun, or on both.

### 6.54 A size-class allocator for the compiler, 2026-10-06

`buri` and the bench now install `buri::allocator` (`cli/src/allocator.rs`)
in place of the system allocator. It answers §6.52's open question.

**The ceiling came first.** mimalloc 3.5.3 from nixpkgs, through
`DYLD_INSERT_LIBRARIES`, against the system allocator. The machine was quiet,
with load 1–6 from these runs alone. Eleven alternating rounds, `buri clean`
before each run. "Large" is `mixed-100k` laid out as a repository twice, once
for node and once for native: 200,000 lines.

| Workload | system, best of 11 | mimalloc | in-tree | max RSS, system | in-tree |
|---|---:|---:|---:|---:|---:|
| example, `build //...` | 0.437 s | −0.9% | +1.8%, median −0.9% | 80 MB | +4.7% |
| example, `build --release //...` | 0.561 s | −2.1% | −0.5% | 93 MB | +5.8% |
| example, `test //...` | 0.236 s | +0.4% | −6.4% | 81 MB | +7.2% |
| example, `lint //...` | 14 ms | −7% | −7% | 35 MB | −0.6% |
| large, `build //...` | 0.583 s | −10.9% | **−12.2%** | 355 MB | −4.3% |
| large, `build --release //...` | 2.587 s | −6.9% | −3.1% | 384 MB | −3.7% |
| large, `lint //...` | 0.246 s | −9.3% | −7.7% | 192 MB | −0.7% |

The mimalloc column is from its own eleven rounds against the system allocator.
The example's `build` best is one fast system run; its medians are 0.456 and
0.452 s.

- **The compiler's share decides it.** The example's wall is mostly `bun`,
  the linker and its tests, so no allocator moves it. The large repository is
  mostly compiler, and there the ceiling cleared the 5% bar.
- **`--release` gains less in-tree** because LLVM allocates through C++
  `malloc`. mimalloc replaces that too; a Rust global allocator can't.
- **The example's lint is 14 ms,** mostly process start, so its 7% is one
  millisecond of rounding.

**How it works.** Blocks up to 1 KiB come from 32 KiB pages, one size class
per page, owned by one thread's heap. Larger and over-aligned blocks go to the
system.

- A thread allocates from its first page for the class until it's spent, and
  frees onto the block's page. Neither touches an atomic.
- A free on another thread pushes onto the page's remote stack with a
  compare-and-swap. The owner takes the whole stack with one swap, so there's
  no ABA. That free also counts itself and queues the page on its heap, which
  is how a spent page comes back.
- A page with no block out goes to a shared pool, for any heap and class.
- An exiting thread leaves its heap for the next new thread to adopt.
  `parallel::map` starts fresh workers for every pass, so this is how their
  pages come back.
- No `Mutex`: the pool and the abandoned list sit behind spin locks. A thread
  that's starting or exiting allocates from the system, and `dealloc` tells
  those blocks apart by address, outside the reserved range.

Per phase on the large `build //...`, CPU seconds, three runs each:

| Phase | system | mimalloc | in-tree |
|---|---:|---:|---:|
| check | 0.052 | 0.044 | 0.044 |
| middle, parallel `lower` and `rc` included | 0.134–0.137 | 0.096–0.099 | 0.095–0.097 |
| emit | 0.471 | 0.317–0.322 | 0.302–0.309 |

**Parallel lowering is faster now, not slower.** The research prototype sent
a block freed on another thread to the freeing thread's list, and `lower` ran
35% more instructions on it. Here it goes back to the page it came from.

**Getting there took three tries:**

- **One free list per class, per thread, and one remote stack per class.** The
  large build was only 5% faster. The middle end ran a third fewer
  instructions than on the system allocator, in the same CPU time. Leaking every remote free
  changed nothing. A variant that never reused a block matched mimalloc. So
  the cost was where reused blocks sat: one list per class scatters a new
  pass's nodes over everything earlier passes freed. Pages fixed it, since a
  page is used up before the next one starts.
- **Pages tied to their class for life.** At exit 241 MB of 252 MB was free,
  pinned to classes nothing still asked for, and max RSS was 28% over the
  system's. The pool brought it under.
- **64 KiB pages.** Same speed as 32 KiB, with 2–4% more RSS.

Also tried and dropped:

- **Classes up to 32 KiB,** on the first design, made no difference to wall
  time.
- **`MADV_FREE_REUSABLE` on every pooled page during a build.** Peak footprint
  dropped from 320 to 228 MB, but only because the reused pages went
  uncounted. With the `MADV_FREE_REUSE` that libmalloc pairs it with, it was
  back at 312 MB.
- **Releasing the pool past 8 MB whenever a page retired.** The peak only
  moved from 314 to 305 MB, because it's pages in use, not pooled ones. It cost
  1.5 points on the large build and lint, and a 32 MB bound cost the same.

**A build's peak footprint is up, and its RSS isn't.** Peak memory footprint,
best of three:

| Workload | system | in-tree | mimalloc |
|---|---:|---:|---:|
| example, `build //...` | 33 MB | 37 MB | 37 MB |
| example, `build --release //...` | 30 MB | 35 MB | 50 MB |
| example, `test //...` | 38 MB | 44 MB | 47 MB |
| large, `build //...` | 200 MB | 299 MB | 288 MB |
| large, `build --release //...` | 198 MB | 297 MB | 328 MB |
| large, `lint //...` | 170 MB | 169 MB | 170 MB |

The system allocator marks freed memory reusable: it stays resident, which is
why RSS is level, but it drops out of the footprint. The in-tree peak is pages
in use. At the large build's end, live small blocks peaked at 82 MB in 139 MB
of pages, the rest being the partly used pages each class keeps per thread.

**An idle process gives its pool back.** `allocator::trim` releases all but
8 MB of the pool, with `MADV_FREE_REUSABLE` on macOS and `MADV_DONTNEED` on
Linux, and `MADV_FREE_REUSE` before a released page is used again. A released
page's bytes may be gone, so the released pages' indices sit in a table of their
own. `buri lsp` trims whenever no request is waiting, and the watch loops trim
after each pass. A build never does, so the wall times above are the same with
it or without it.

`buri lsp` on the large repository, 40 edits to one literal with a diagnostic
pull after each, then five seconds idle, three runs each, all identical within
1 MB:

| | system | in-tree, no trim | in-tree |
|---|---:|---:|---:|
| the 40 edits | 4.15 s | 3.77 s | 3.84 s |
| footprint, idle | 152 MB | 173 MB | 149 MB |
| RSS, idle | 230 MB | 194 MB | 194 MB |

Trimming costs the burst 2% and brings the idle footprint level with the system
allocator's. Linux wasn't measured: there `MADV_DONTNEED` drops RSS itself.

**The bench installs it too,** so its timed rows from here on measure the
allocator `buri` ships with. Expect a step in every series: 20–30% fewer
instructions in check, the middle end and emit. `--features alloc-counter`
still swaps in the counting allocator over the system's.

**Validation:** the workspace suite (2,481 tests, 142 s with the build),
`--features backend-llvm --test native`, CI's heap-check step, clippy, and two
hundred runs of the allocator's own tests. Those run 16 threads that hand
blocks to each other to free, realloc across classes, and adopt each other's
heaps, plus checks that pages emptied by remote frees serve another class and
that trimmed pages come back intact.

`a_dial_to_a_port_nobody_holds_is_refused`, in `cli/runtime/net.rs`, failed
twice in about 210 runs of the runtime's tests under the heap check. Those
don't link this allocator, and the failing run's detail wasn't kept, so the
cause is open.

### 6.55 `format --check`, owned list splices, and two issues already fixed, 2026-10-06

**`buri format --check` (#259).** A no-op check formatted every file, one
after another, every run. Now each file is laid out on a worker, and its
verdict is kept in `.buri/cache`:

```text
key     = H("format", toolchain, "in-process", settings, path, bytes)
verdict = changed, has a syntax error, refused
```

`settings` is `buri` for sources and build files, `document` for markdown, and
the language's kind for a referenced JSON, proto or textproto file. A
repository's own language still goes to its tool, in order, on the main
thread. `buri format` lays out again any file whose verdict says it changes,
because only the verdict is kept. A build file that doesn't parse is never
kept, so it stops the command every time.

#259's repository, 150 libraries and 8.8 MB, release, at load 14–17 on 10
cores:

| `format --check` | Before | After |
|---|---:|---:|
| no-op | 0.29–0.39 s real, 0.27 s user | 0.03–0.04 s real, 0.01 s user |
| nothing remembered | 0.32–0.34 s real | 0.10–0.11 s real, 0.49 s user |
| one 50 KB path | 0.02 s | 0.01–0.02 s |

The issue measured 1.3–1.5 s on 0.3.22. The formatter got faster since then.

Output is unchanged. On all 416 checked-in repositories, `format --check` and
`format` print the same stdout and stderr, exit the same way and leave the
same tree as before, with nothing remembered and after a remembered pass.
`cli/format_check_remembers` holds that through an edit after a clean pass, a
file put back, `buri clean` and `format` after `--check`.
`profile::a_second_format_check_formats_no_file` holds a warm check to 0
`files formatted`, a new `BURI_PROFILE` line.

`buri gen --check` has the same shape (the issue's comment) and is untouched.

**Owned list splices (#251).** §6.48 made `core/map`'s splices runtime calls
that own their list. `[T].replaceAt`, `insertAt` and `removeAt` still built
three lists a call. They now run `core/map`'s bodies:

```rust
via("map.replaceAt", e("list.replaceAt", &[Elems, Dropped, Scalar, Spilled, Stride, Retain, Release, Equal], Ret::Out)),
```

The list comes first instead of second, and `Dropped` takes no C argument, so
both rows flatten to the same C call. `TAKEN_NATIVELY` hands them the receiver.
JavaScript copies.

#251's repro, ns per operation, two runs each at load 13–26:

| | `main` | After |
|---|---:|---:|
| `--release`, owned `[Str]` len 32, `replaceAt` | 160–274 | 7 |
| `--release`, owned `[Str]` len 1024, `replaceAt` | 4,583–7,595 | 8 |
| `--release`, owned `[Int]` len 1024, `replaceAt` | 274–398 | 5–7 |
| debug, owned `[Str]` len 1024, `replaceAt` | 9,413–11,990 | 11–52 |
| `push` on the same list | 13–18 | 13–18 |
| `--release`, `Map<Int,Int>` insert, 1M | 190–194 | 201–211 |

The issue's 1.1–2.3 µs map insert was §6.48's.

**A stencil miscompile on the way.** `rt_call` zeroes a `Ret::Out` destination
before the call, and the call reads its arguments from the frame when it runs.
A list that dies at the call can share its slot with the destination:

```buri
fn f<C: Allocator>(ctx: C, xs: [Str], n: Int): [Str] {
    if (n == 0) { xs } else { f(ctx, xs.replaceAt(ctx, 16, "r"), n - 1) }
}
```

The splice read an empty list, answered one, and leaked the original. Each
argument word that overlaps the destination is now read into its argument word
first. `core/map`'s rows had the same exposure but never met that slot reuse.
The 208 objects of the eight saved corpora, the ten shapes, `cli/tests/example`
and #251's repro are byte-identical with and without the fix.

`native::ownership`'s `a_list_the_caller_owns_is_spliced_in_place` holds 6,000
splices of an owned list under 50 blocks, down from 20,005. `native::agreement`'s
`a_list_spliced_through_one_name_is_unchanged_through_another` runs shared lists,
indices past either end and tail-call splices on JavaScript, stencil and LLVM
under the heap check.

**Already fixed:**

- **#252, JavaScript `popCount`**, by §6.53 (`53d370bbd`). On the issue's
  repro under node 22.23.3, best of three, a get on the 100k map went from
  1,793 ns with the old bit loop to 535 ns, and an insert from 2,325 to 1,243.
  `the_runtime_counts_bits_without_a_loop` now fails if a bit count loops.
- **#253, native no-op builds**, by §6.43 (`d9f430860`). On the issue's
  160-library repository a native no-op takes 0.02–0.04 s, against 0.41–0.43 s
  in the issue. `--release` and `//libs/l159` are the same, the node no-op is
  0.02 s, and no `cc` is spawned. `profile::a_warm_native_build_loads_no_module`
  fails with the record lookup turned off, which loads 4 modules.

### 6.56 Debug glue reads a wide struct where it is, 2026-10-06

buri-lang/buri#255. A debug build's retain, release and copy glue copied the
whole value into its frame before touching a field. For a struct of 64 `Str`s
that was a 1536-byte `memcpy` into a 1840-byte frame, then 64 pointer
operations. A list's element glue did the same per element.

Now the glue copies a value whole only when one fixed-width `eload` covers it:
64 bytes or less, at a width the library has. Anything wider is walked through
its address (`stencil/glue.rs`):

```text
ldr  x10, [x0]          ; the value's address, from the glue frame
ldr  x9,  [x10, #off]   ; one pointer, tag or niche word
str  x9,  [x0, #0x20]   ; where the incref/decref stencil reads it
```

- **Retain and release** load only the words the walk counts or tests, by
  `walk_rc`'s own rules. A deep, heavy field still goes to its type's glue, by
  address now.
- **Copy** loads each counted field on its own and writes it back. A field
  wider than one load goes to its type's glue by address.
- **A list's element glue** tests a wide element for headroom where it sits,
  then hands its address to the element type's glue.

So no glue frame holds more than 64 bytes of value. The 64-field struct's glue
frames went from 1840 bytes to 320 or 336, and #246's frame limit can't be reached by
glue any more.

The issue's repro, `genbig.py`, best of five alternating runs at load 18–27.
Debug before is `ecc3749fc`:

| Struct | Debug before | Debug after | `--release` |
|---|---:|---:|---:|
| 4 `Str` fields | 18 ns | 17 ns | 8 ns |
| 16 `Str` fields | 45 ns | 42 ns | 20 ns |
| 64 `Str` fields | 195 ns | 187 ns | 87 ns |

A 64-field step retires 2,893 instructions before and 2,635 after (−9%),
against 1,438 in release. The whole repro retires 4.03 G before and 3.71 G
after.

**The copy wasn't most of the cost.** A 1536-byte `memcpy` from L1 takes a few
dozen cycles, and the three loads per field that replace it give some of that
back. In a profile of the 64-field loop, glue is now about 40% of the samples,
and nearly all of it is the per-field `incref` and `decref` stencils: about 11
and 20 instructions each, with a prologue and register zeroing the C compiler
puts around `decref`'s call to `buri_rt_free`. The rest of the step is the
list literal, which still copies the struct into the block, and the
allocation.

**The bytes didn't move except the glue.** All 270 objects of the eight saved
corpora, emitted in-process for `macos-arm64`, `linux-arm64` and
`linux-x86_64`, are identical. None of them has a value this wide. Of the
18-binary repository's 172 objects, 156 are identical. In the other 16, every
function but the glue disassembles the same once addresses are left out, and
the constant pool differs only in where it starts and in 16 bytes of trailing
padding.

**Not done:**

- **A stencil that counts through a pointer**, such as `incref` reading
  `*(AT(A) + OFF(N))`. It would save the store and reload, two of about 14
  instructions per field. It changes the stencil library, and with it every
  build's identity, for about 10% of the glue.
- **A shorter `decref`.** The prologue is clang's. Moving the dying arm out of
  line would change every object that releases anything.

### 6.57 Proto codecs write into one buffer, 2026-10-07

[#254](https://github.com/buri-lang/buri/issues/254): a generated encoder built
a list per field and per varint byte, and copied each nested message once per
level. The decoder sliced every nested message out before reading it.

The encoder appends to one `[U8]` now, and a message's `size` function gives
the length prefix a parent writes before it:

```buri
export fn writeItem<C: Allocator>(ctx: C, out: [U8], value: Item): [U8] {
    let out = match (value.id) {
        .Some(x) => proto.writeVarint(ctx, out.push(ctx, 8), x),
        .None => out,
    };
    …
    match (value.tag) {
        .Some(x) => writeTag(ctx, proto.writeVarint(ctx, out.push(ctx, 26), sizeTag(x)), x),
        .None => out,
    }
}
```

A header is a constant, so it's pushed as literal bytes. The decoder reads
between an offset and an end, `readItem(ctx, b, at, end, base, acc)`, and an
error counts its offset from `base`, so it still names the offset inside the
message it was found in.

Sizing first exposed `str.utf8Length`, which walked a string one `charAt` at a
time: half of a native encode. It's an intrinsic now. Natively it's the byte
length a `Str` already carries, and JavaScript counts UTF-16 units.

Instructions retired per message, for #254's 262-byte page, less a run that
builds the page and stops. "Push" is #254's floor: 262 bytes pushed into one
`[U8]`.

| | before | after | push |
|---|---:|---:|---:|
| native `--release`, encode | 66,322 | 31,364 | 35,221 |
| native `--release`, decode | 47,918 | 35,664 | |
| native debug, encode | 107,734 | 42,716 | 44,981 |
| native debug, decode | 96,186 | 72,394 | |
| node `--release`, encode | 279,106 | 75,306 | 27,337 |
| node `--release`, decode | 318,301 | 180,447 | |

Encode is under the floor natively, because a string goes in as one `concat`
rather than a push per byte. #254's own program, one run each at load 9:
native `--release` encode 3.2 µs to 1.4 µs and decode 3.2 µs to 1.4 µs; node
encode 16.2 µs to 3.3 µs and decode 15.1 µs to 8.2 µs. `cli/tests/conformance/lib/proto/test/wire.buri`
pins the bytes and the error offsets on JavaScript and stencil.
`build::generators` runs a nested page through stencil and LLVM.

**What's left:**

- **A string field.** Decode still slices its bytes out, and the native
  `bytes.fromUtf8` builds the string one scalar at a time. That's most of a
  native decode.
- **`BigInt`.** On node, 35% of a decode is the collector, behind index
  arithmetic and the struct update each field makes. 10% of an encode is
  `$wrapTo`, which takes `wrapToU8` through `BigInt.asUintN`.
- **A nested message is sized once per level above it.** The page sizes each
  tag twice. A schema nested ten deep pays ten times; nobody's asked for that.

**Considered and not built:** reserving a byte for the length and filling it
in after the body. §6.55's `replaceAt` writes in place natively, but
JavaScript copies, and a body of 128 bytes or more needs a second byte anyway.

### 6.58 A constant `ctx`, measured and left, 2026-10-06

The idea: a program has one or two real contexts, so compiled code could treat
a known one as a fixed address and stop passing `ctx`. There's nothing left to
win. Monomorphization already did it, more strongly than a constant would.

```text
; release, spin<C: Random> at context { …, Random: Zero {} }
spin:  subs x9, x1, x0        ; x0 = i, x1 = n, x2 = acc. No ctx.
```

- **Every effect call is a direct call.** `resolve_trait_call` reads the
  implementation's type out of the context type, so a call through `ctx` is a
  `GetField` and a direct call, never a table. JavaScript does the same: a
  context is an array, and a call is `c[k]` passed to a known function.
- **A host context isn't passed at all.** All twenty `Host*` implementations
  are empty structs, so the context is zero-sized and the layout pass drops it
  from every signature (VALUE-MODEL.md §8). That's every production `main` in
  `cli/tests/example`: `server`, `web`, `basket` and `tools/report` build one
  context each, and none of them weighs anything. `server.serve` hands each
  request's handler the `ctx` it was given and builds none.
- **A context with state is a register.** `Mul(k)`, a one-word `Random` built
  at run time, keeps `k` in `x0` for the whole loop. The release loop is ten
  instructions, the same ten as the hand-written one.
- **Tests are the only contexts with state, and they aren't constants.** A
  double is a handle the runner hands out (`TestStdout(I64)`), so each test's
  context differs at run time. §6.12 already folds 126 test contexts into one
  instance per list of bindings.

A microbenchmark: a tail-recursive `spin<C: Random>` in a library, 300 M
steps of `acc + random.int(ctx, i, acc)` against a hand-written
`next(i, acc)`. Instructions retired per step, best of three. JavaScript runs
30 M steps under bun.

| `ctx` | native `--release` | native debug | bun `--release` |
|---|---:|---:|---:|
| none, hand-written | 10.1 | 18.1 | 1,886 |
| `Zero {}`, zero-sized | 10.1 | 18.1 | 1,890 |
| `Mul(31)`, constant | 10.1 | 21.1 | 1,893 |
| `Mul(k)`, built at run time | 10.1 | 21.1 | 1,893 |

Logging in a loop, 2 M `io.println`s through a host context: 789 instructions
a line in release and 931 in debug, none of it `ctx`.

**Debug's three extra instructions are copies, not passing.** Stencil copies
`ctx.Random` to one slot and `.0` to another before the `mul`
(`ldr/str/ldr/str/ldr`). A constant would trade the first load for `adrp` and
a load. Passing a `{ alloc(), fs() }` test context costs a 32-byte frame
copy per call in debug, four instructions: 21% of a call that does nothing
else.

**Not done:** context specialization. It would have nothing to remove in
release and only stencil's copies to remove in debug. Two smaller changes would
reach those, and neither is about contexts: forwarding a `GetField` out of a
one-field struct, and passing a wide aggregate to a stencil callee by address.

**Done since: a field is read where it is.** `jit.rs::forward_fields` gives a
`GetField`'s result the bytes of the field inside its struct's slot, so the
load and store that copied it out become the identity:

```text
; spin, debug, after
ldr  x8, [x0, #0x8]     ; ctx.Random.0, read in place
mul  x3, x8, x1
```

It needs the struct's bytes to keep still while the field is read. Either the
struct is written once (a slot class of its own, not a block parameter, inside
the frame), and the field may be read anywhere, or every read is a later
instruction of the same block, within 32, and nothing up to the last one writes
those bytes. That covers any struct, not only one field: a field of a record
parameter is forwarded too.

Instructions retired per step, best of alternating runs. The climbs are
`random.int(ctx, n, 1) + climb(ctx, n - 1)`, 30 M calls:

| Debug | Before | After | |
|---|---:|---:|---:|
| `spin`, `Mul(k)` built at run time | 21.1 | 17.2 | −18% |
| `spin`, `Mul(31)` | 21.1 | 17.1 | −19% |
| `spin`, `Zero {}`, and the hand-written loop | 18.1 | 18.1 | 0 |
| climb, `Mul(k)` | 34.9 | 30.9 | −11% |
| climb, a 24-byte `Random` | 42.9 | 34.8 | −19% |

The workloads didn't move outside their noise. `cli/tests/example`'s
`buri test //...` ran its tests in a median 895 M child instructions before and
889 M after, over eight cold runs each with a spread of 10%. A generated repo of
200 libraries and 400 tests read 3,432 M and 3,444 M, spread 1.5%. Neither
`emit` moved. The 18-binary repository's `emit` stayed at 371–385 M, and its
binaries, which are mostly startup, ran 300.3 M before and 299.2 M after.

**The bytes moved only where a copy went.** Of the 270 objects of the eight
saved corpora on all three targets, 120 are identical. In the other 150, every
one of the 2,175 functions that differs is its old self with frame copies taken
out (17,270 instructions) and offsets and branch targets changed. Nothing else
changed and no function grew. The 18-binary repository is the same: 112 of
172 objects identical, and 267 functions lose 14,332 instructions.

**Measured and left: a wide context passed by address.** The prototype passed
a context wider than 16 bytes to a function with a body as the address of the
caller's slot. A callee that only hands it on keeps the address. Any other read
copies the value into a home on entry. Per call, best of alternating runs:

| Debug, on top of forwarding | By value | By address | |
|---|---:|---:|---:|
| `fan` over a 24-byte context, which only hands it on | 25.2 | 21.2 | −16% |
| the `{ alloc(), fs() }` `fan` test, both tests' child instructions | 2.85 G | 2.71 G | −5% |
| climb, a 24-byte `Random`: one effect call per call | 34.8 | 42.2 | +21% |

A copy only moves. Where the callee reads the context, the caller's copy
becomes the callee's, with a `lea` and an `eload` on top, and most callees read
their context. Reading fields through the pointer instead needs a `pload`
stencil the library doesn't have, and costs about three instructions per read
against the two to four a call saves. That's even at one read and a loss past
it. The example and the generated repo stayed inside their noise either way.

### 6.59 `gen --check`, 2026-10-06

[#260](https://github.com/buri-lang/buri/issues/260): a no-op `buri gen --check`
cost about 0.9 s per MB of source, on one core, every run. `gen` analysed
each package over its whole closure, one package at a time, so a chain of 150
libraries checked its first library 150 times. On #260's repository, `check`
was 76.5 G of 97.6 G instructions.

**One compilation.** The packages `gen` works out are checked together
(`driver::shared`), as `buri lint` already does (§6.47). Each target reads its
own share: the modules it holds, what it reported, and its own bodies. Where
the shared check can't promise each target its own answer, it declines and
each target is analysed alone, as before.

**One record per package.** Whether a build file is out of date is kept in
`.buri/cache`:

```text
key    = H("gen", toolchain, graph, package, its .buri and .proto files)
record = stale or same
         each generator rule it loaded, with the key it ran under
         each file it read, with a digest
```

`graph` is `Sources::graph_key`: which files exist, and every build file's
bytes. The files read are each target's closure (`sources::closure_over`)
plus any file whose imports `gen` read off disk. A generator's key covers its
tool's program, including the files no rule lists (§6.43). Each digest is
taken once a run, on every core. A remembered "stale" that `gen` is going to
write gets worked out again, because only the verdict is kept. Errors are
never kept.

#260's repository, 150 libraries and 8.8 MB, release, at load 30–57 on 12
cores:

| `gen --check` | Before | After |
|---|---:|---:|
| no-op | 6.00–18.47 s real, 5.66–8.04 s user | 0.04–0.06 s real, 0.02 s user |
| nothing remembered | the same | 0.24–0.29 s real, 0.15–0.17 s user |
| no-op, 0.8 MB | 0.77–1.68 s | 0.05–0.06 s |
| no-op, `//libs/l150` | 0.14–0.53 s | 0.02 s |

Instructions: 97.6 G before, 3.1 G with nothing remembered, 0.5 G for a no-op.

Output is unchanged. On all 416 checked-in repositories, `gen --check` and
`gen` print the same stdout and stderr, exit the same way and leave the same
tree as before, with nothing remembered and after a remembered pass.
`cli/gen_check_remembers` holds that through a dependency's body edited after
a remembered pass, a new source, a hand-edited build file, a generator's tool
edited, `buri clean` and `gen` after `--check`. Dropping the closure from the
record fails its first edit. `profile::a_second_gen_check_works_out_no_build_file`
holds a warm check to 0 `build files worked out`, a new `BURI_PROFILE` line,
and 0 modules loaded, and `a_second_gen_check_runs_no_generator` does the
same with a generator in the tree. `a_cold_gen_check_of_a_chain_of_libraries_loads_each_once`
holds a cold one linear in the chain: it loaded 5,050 modules at 100
libraries and 20,100 at 200 before, and 100 and 200 after.

**What's left:**

- **Any build-file edit forgets every record**, because the graph is in the
  key. The next check is a cold one: a quarter of a second here.
- **A record lists its whole closure.** A chain of 150 writes about 2 MB of
  records. §6.43's closure nodes would list each file once.
- **A tree the shared check declines**, say one import of a module that
  doesn't exist, is analysed a package at a time and is as slow as before
  when nothing is remembered.

### 6.60 The slowest `test` blocks, 2026-10-07

Every `test "…"` block in the repository, timed by `buri test --verbose`,
ranked, and the top ones taken apart. A release `buri` from `f9d0deaaa`, each
corpus copied out of the tree so the runs leave it alone:

```text
cp -R cli/tests/conformance /tmp/conf && cd /tmp/conf
buri test //... --verbose --force                     # JavaScript: every BUILD.buri says backends: [JS]
sed -i '' 's/^ *backends: \[JS\]$//' lib/*/BUILD.buri
buri test //... --verbose --force                     # native, on stencil

# the same pair in every repository under cli/tests/repositories, cli/tests/example,
# cli/tests/tutorial and cli/tests/docs/repositories that holds a test block
buri test //... --verbose --force
buri test //... --verbose --force --output=js
```

`--force` makes every suite run rather than report as cached. Each time below
is the **best of three or four runs** at load 30–50, because one run swings
wildly: `the type properties, on a heading` took 215 ms once and 5.7 ms on all
three reruns. Instructions come from `/usr/bin/time -l` on the suite's own
binary under `.buri/out/native/<host>/`, or on
`bun .buri/out/node/<package>/test-library.mjs` with every block index on
standard input.

**Where the slow tests are.** The conformance corpus holds all of them; seven
of its packages don't compile natively and run on JavaScript only. No
repository fixture has a test over 42 ms best-of, on either backend. The
standard library has no `test` blocks of its own: `core/*` is tested by the
conformance corpus. The docs harness type-checks its `role=test` fences and runs
none. `--release` needs LLVM for a native run, and on JavaScript it moved none
of the rankings.

Before, best of three:

| Test | Native | Share of suite | JS | Share of suite |
|---|---:|---:|---:|---:|
| `crypto` · the million-character message | 153 ms | 93% | 2.1 s | 81% |
| `collections` · a map fromSorted survives inserts and removes either side of every level boundary | 90 ms | 47% | 563 ms | 33% |
| `data` · normalization vectors, 17 blocks of 60 | 16–18 ms each, 269 ms in all | 4–5% each | 68–254 ms each | 2–9% each |
| `collections` · a map fromSorted survives inserts and removes at every size up to 130 | 21.5 ms | 11% | 125 ms | 7% |
| `collections` · fromSorted answers like inserts at every size up to three hundred | 18.4 ms | 9.5% | 160 ms | 9% |
| `compression` · a match at the window's last distance is taken, and one past it is not | 14.6 ms | 49% | 185 ms | 33% |
| `compression` · six kilobytes there and back, through both wrappers | 5.3 ms | 18% | 157 ms | 28% |
| `collections` · ten thousand entries round-trip through get | 5.8 ms | 3% | 149 ms | 9% |
| `ui/every_shape_and_state_is_painted` · every property at the size of the page | 41.6 ms | 36% | — | — |
| `ui/every_shape_and_state_is_painted` · a thousand siblings | 40.3 ms | 35% | — | — |

One or two tests make up most of four suites: `crypto` (94% native, 84% JS),
`compression` (67%, 62%), `proto` on JS (65%, `every varint is the bytes
core/bytes writes for it`), and `every_shape_and_state_is_painted` (71%).

**Three fixes**, each in what the tests call rather than in the tests:

- **`core/str` decoded its Unicode tables on every call.** `normalize`,
  `caseFold` and `graphemes` turned each table into a `[Char]` first, about
  86,000 characters for NFKC, so a one-character string paid for all of them.
  Every table is ASCII now, value tables included, so a probe is `charAt` on a
  literal, an index under VALUE-MODEL.md §3.1's ASCII flag. Hangul is UAX #15's
  arithmetic rather than an 11,172-entry table. `native::strings::a_short_string_normalizes_without_copying_the_unicode_tables`
  holds seven hundred calls under 1 MB allocated: 120.6 MB before, 89 KB after.
- **`core/orderedmap` built each node three times.** Its splices were `take`,
  `push`, `concat` and `drop`. They are `core/list`'s `insertAt`, `replaceAt`
  and `removeAt` now, which copy a node once, or write into it where nothing
  else holds it, as `core/map`'s do. `native::collections` holds four thousand
  edits under 30,000 blocks, 58,289 before and 22,317 after, and checks that an
  edit leaves every other name for the map as it was.
- **JavaScript shifted a `U8` or `U32` through `BigInt`.** Those widths are
  `number`s, and JavaScript's own 32-bit shifts are the answer at both. SHA-1
  and SHA-256 live on them.

| | Before | After |
|---|---:|---:|
| `//lib/data`, native | 6.91 G, 381 ms | 0.90 G, 62 ms |
| `//lib/data`, bun | 39.0 G | 6.3 G |
| normalization vectors, 0 to 60, native | 15.9 ms | 1.7 ms |
| `//lib/collections`, native | 2.32 G | 1.72 G |
| `//lib/collections`, bun | 16.3 G | 14.2 G |
| either side of every level boundary, native | 90 ms | 49 ms |
| `//lib/crypto`, bun | 42.8 G | 37.7 G |
| the million-character message, JS | 1.7 s | 1.5 s |
| conformance, sum of suite times, native | 807 ms | 460 ms |

**Slow by design**, and left as they are:

- **The million-character message** is FIPS 180-4's long vector: 7,813 SHA-512
  blocks and 15,625 SHA-1 blocks, and the length is the point. It can get
  cheaper without getting shorter, in two places:
  - `core/crypto` builds its 80 round constants and copies them once per
    block, slices each block out of the message, and rotates with two shifts
    and an or. Built once per digest, with `bits.rotateRightU64`, SHA-512 of a
    million bytes drops from 0.96 G to 0.68 G instructions, measured on a copy.
  - Stencil calls `number.U64.wrappingAdd`, `bits.rotateRightU64` and the
    shifts out of line. `emit.rs` open-codes them only as the callee's body. In
    the copy above they are 29% of the samples, before the call overhead
    around them. Emitting them at the call site is the next step.

  On JavaScript a `U64` is a `BigInt`, and `BigInt.asUintN` alone is 16% of
  the profile.
- **The level-boundary test** needs maps of 4,094 to 4,097 entries to reach the
  third level, and drains each from both ends. What's left is path copying:
  the retain walk over each node copied was a quarter of its samples before
  the fix.
- **The normalization vectors** are Unicode's own 999 cases, four forms each.
  The 840–960 blocks still take 8–10 ms natively: they carry runs of combining
  marks, and the canonical reordering looks up every mark's class on every
  pass.
- **The deflate window test** round-trips two 32 KB inputs, because the
  window is 32 KB.
- **The two page-sized pictures** paint an 800 × 600 scene of every property
  and of a thousand siblings.

**What's left:**

- `core/crypto`'s changes above, and open-coding the numeric and bit
  intrinsics in stencil.
- JavaScript's `$starts` keeps one string's scalar table, and `normalize`
  alternates between four tables. That miss is 8.7% of `//lib/data` on bun.

### 6.61 Ordered-map lookups, `filterMap` and `values`, 2026-10-07

Three issues, one cause each.

**A lookup copied every entry it passed (#267).** A node held its entries as
one `[(K, V)]`, and the search read each entry it compared a key against:

```buri
match (entries.get(at)) {         // the whole (K, V), into an Option
    .Some(e) => {
        let (k, _v) = e;          // every counted field in V retained, then released
        …
```

So `has` on a map of 64-`Int` values cost three times `has` on one `Int`. A
node keeps its keys apart from its values now, and only a leaf has values:

```buri
struct Node<K, V> {
    keys: [K],               // a leaf's keys, or a branch's separators
    values: [V],             // a leaf's values; a branch's are empty
    children: [Node<K, V>],  // a leaf's are empty
}
```

It's a B+ tree: a separator is a copy of the first key to its right. A search
reads keys alone and always ends in a leaf, `has` reads no value, and `get`
copies the one it answers.

**`filterMap` grew its answer a push at a time (#266).** It was a `foldCtx`
through a lambda that captured `transform`. A capturing lambda runs through its
thunk, which owns each element it's handed, and the fold pushed each payload it
kept. When a push outgrows its block, `append_dest` (`cli/runtime/list.rs`)
copies into a bigger one and retains every element, and the old block releases
them when it dies. `filterMap` and `filterMapCtx` are list loops now, like
`filter` (`lower/lists.rs`, `Step::FilterMap`): one block the length of the
list, each `.Some` payload moved in with the count the step gave it, and the
block cut to what was kept. JavaScript runs the same loop in `runtime.js`.

**`values` mapped the entries (#266).** It built `entries` and mapped them,
which retained each value twice. With every value in a leaf, `values` and `keys`
`flatten` the leaves' own lists, which sizes the answer once.

Instructions a lookup on a map of 2,000 `Int` keys, from
`native::collection_costs`:

| Value | stencil `has` | stencil `get` | LLVM `has` | LLVM `get` |
|---|---:|---:|---:|---:|
| one `Int` | 2,249 → 2,009 | 2,286 → 2,090 | 526 → 428 | 528 → 435 |
| 64 `Int`s | 6,238 → 2,009 | 6,402 → 2,768 | 807 → 428 | 809 → 680 |
| eight `Str`s | 8,396 → 2,010 | 8,791 → 2,919 | 3,363 → 425 | 3,324 → 628 |

#267's program on a map of 100,000 entries, ns a lookup, best of three at load
3–7:

| Value | debug `get` | debug `has` | `--release` `get` | `--release` `has` |
|---|---:|---:|---:|---:|
| one `Int` | 237 → 220 | 229 → 201 | 44 → 43 | 45 → 43 |
| 64 `Int`s | 675 → 280 | 654 → 225 | 59 → 58 | 61 → 48 |
| eight `Str`s | 582 → 254 | 567 → 216 | 171 → 48 | 173 → 46 |

Instructions an element over 2,000 records of four `Str`s, four `Int`s and a
`[Str]`, #266's shape:

| | debug | `--release` |
|---|---:|---:|
| `map` | 426 → 425 | 144 → 143 |
| `filterMap` | 1,260 → 497 | 737 → 171 |
| `values` | 1,374 → 456 | 730 → 189 |

#266's program at n = 100,000, ns an element, best of three:

| Element | Operation | debug | `--release` |
|---|---|---:|---:|
| `Wide` (4 `Str`, 1 `[Str]`, 4 `Int`) | `list.map` | 17 → 17 | 6 → 6 |
| | `list.filterMap` | 63 → 23 | 30 → 9 |
| | `OrderedMap.values` | 48 → 17 | 27 → 9 |
| `Flat` (9 `Int`) | `list.map` | 7 → 7 | 1 → 1 |
| | `list.filterMap` | 25 → 11 | 10 → 2 |
| | `OrderedMap.values` | 13 → 10 | 6 → 3 |
| `Int` | `list.map` | 1 → 1 | 0 → 0 |
| | `list.filterMap` | 10 → 2 | 5 → 0 |
| | `OrderedMap.values` | 4 → 4 | 2 → 2 |

**What an edit pays.** A node is two lists where it was one, so a copied path
allocates more. `native::collections`' four thousand edits allocate 26,444
blocks rather than 22,317, and retire 36.1 M instructions rather than 33.7 M in
`--release`. Two thousand inserts, two thousand removes and two thousand
persistent inserts: 40,599 blocks rather than 34,496, and 54.6 M instructions
rather than 50.2 M.

`native::collection_costs` holds `has` on 64 `Int`s and on eight `Str`s within
1.2 times `has` on one `Int`, and what `get` adds over `has` to the same in one
leaf and three levels down. It holds `filterMap` and `values` within 1.3 times
`map`. Each bound failed before its fix. The kernel counts the instructions, and
CI's virtual machines have no counter, so there the test checks only the answers
and that no lookup allocates.

**What's left:** a push that outgrows its block still retains every element it
moves. Moving them needs the push to own its receiver when the receiver dies,
which is a change to `middle::rc`'s plan.

### 6.62 A suite named alone runs its tests side by side, 2026-10-07

Issue #265. Eight tests of about a second each, from the issue:

```sh
buri test --force //libs/burn                    # 9.2 s wall, 9.2 s CPU
buri test --force //libs/burn //libs/greeting    # 1.5 s wall, 10.2 s CPU
```

`--verbose` showed every test of the lone run taking its full second, one after
another. The cause was in `cli/src/commands/test.rs`. Two or more uncached
suites go to `batch`, and a batch member's blocks are handed to several
processes that pull them one at a time (`queue_members`, `run_pulled`). A
single suite skipped `batch` and went to `run_solo`, which ran every block in
one process with `run_blocks`. A `--filter` that narrowed the run to one suite
took the same path.

`run_solo` now links as before and hands the binary to `queue_members`, as a
batch member gets. That keeps everything a member already has:

- `--jobs` caps the processes, and a run holds no memory budget.
- Verdicts gather by block index, so the report keeps declaration order
  whichever process finished first.
- A suite with `timeout_seconds` keeps one process, the same rule that keeps
  it out of a batch, so its limit still bounds the whole suite.
- Two suites never paint into one snapshot directory at once.

The one difference is a process that ends without a verdict: a failed heap
check, or a death after the last block. A batch member goes back to be built
alone, and a suite on its own already is. So it runs again in the same binary,
one process at a time (`run_alone`), and the diagnostic names the suite exactly
as before.

Tests may run in any order and share nothing (the header of `test.rs`,
TESTING.md "Running"). Each builds its own context, so running them side by side can't
change a verdict. The batch path has always run them that way.

**A `left` line named the wrong block under `--filter`.** The runtime's
`buri_rt_test_leave` gets the test's index in the checked program, and wrote
that into the `left` line. `enter` and the runner number blocks by position in
the binary, and a filtered binary leaves gaps. So `--verbose --filter` showed
one test with no time and another with its neighbour's. `leave` now writes the
index `enter` recorded. The pulled runner also reads `left` to tell a death
after a block from one inside it, so that attribution was off under
`--filter` too.

**Painting stays in order.** Two blocks that name one snapshot share its file,
and the later block's picture is the one `--update` keeps
(`ui/a_snapshot_that_cannot_be_compared` holds this). Processes painting side
by side made that a race. The batch path had the same race, on a premise that
one suite's processes write different files. So a suite that paints runs one
process at a time, batched or alone, as a suite with a limit does.

**A binary that cannot start is started once.** Helpers launch only after a
process has reached its first block, so an unloadable binary isn't launched by
every helper at once. A lone suite whose process never started reports that
without launching again.

The issue's repository, debug `buri`, `--force` after a warm build, on a
10-core machine shared with other agents at load averages of 10 to 45. Best of
three to six runs:

| Command | Before wall | After wall | User CPU |
|---|---:|---:|---:|
| `buri test //libs/burn` | 14.7 s | 3.9 s | 8–10 s either way |
| `buri test --filter=spin //libs/burn` | 7.7 s | 1.4 s | 8–10 s |
| `buri test //libs/burn //libs/greeting` | 2.6 s | 3.9 s | 10 s |

The lone run now matches the two-target one. The two-target row didn't change
in code, so its spread is the machine's.

The repository's own `repositories::` tests took 52–59 s of user CPU before and
after. Their wall time swung from 51 s to 288 s between back-to-back runs of
one binary, so no wall-time change could be read from them. Their suites
mostly hold tests of microseconds, where one process was already enough.

The tests are in `cli/tests/build/scheduling.rs`.
`a_suite_named_alone_runs_its_tests_side_by_side` wraps the test binary in a
script that writes `start` and `end` around it and holds the binary's output
until a second process starts, so it fails with one process at a time
whatever the load. `a_suite_named_alone_reports_as_it_did_one_process_at_a_time`
pins the report, the `--verbose` list and the `--filter` list as the old runner
printed them, apart from the `--filter` time it used to drop.

### 6.63 A test runner runs from the first file that held its bytes, 2026-10-07

`repositories::snapshots` took 332 s of a 7-minute suite. It isn't a
regression. Alone, `origin/main` took 50–224 s and `c2e568faa`, before the
#261–#264 cases, 51–124 s, at load 5–20. CPU stayed at about 58 s in every run;
the spread is the wait. The three new cases add 7 of the corpus's 303 runs.

The wait is §6.39's check. `sample` found the runners at `_dyld_start` for 14 s
while `syspolicyd` checked them. A run links 189 runners, each one distinct,
and the next run links the same 189 bytes again into fresh scratch
repositories. `buri test` alone in a copy of `sweep_animation` took 3.0 s with
0.2 s of CPU, and 0.12 s again in the same copy.

`build/programs.rs` keeps the first file to hold a runner's bytes in
`~/.buri/programs/<sha256>/program` and makes the runner a symbolic link to
it, the same as the harness's `kept::settle` does for the programs it links.
macOS only, best effort, and entries go after two hours unused.

| | `origin/main` | After |
|---|---:|---:|
| `repositories::snapshots` alone, store warm | 50–224 s | 10 s, 13 s |
| Workspace suite, first run | 300 s, load 49 → 6 | 226 s, load 7 → 13 |
| Workspace suite, rerun | 218 s, load 6 → 7 | 67 s, load 13 → 58 |

The first run after a change that moves codegen, the runtime or the standard
library still links new bytes and pays every check.

`work_counts::a_runner_an_earlier_run_started_is_not_a_new_executable` pins it:
the same tree in a second repository, and again after `buri clean`, launches
no new executable. The other scenarios each get a `BURI_HOME` of their own, so
what an earlier test kept can't move their counts. A runner that is now a
symbolic link is one regular file fewer written per link.

### 6.64 Wide integers hash every bit, 2026-10-08

A `core/map` of 16,000 `Int` keys `i * 2^32` took 4.1 s to build on
JavaScript, against 27 ms for keys `i` (#273). Every integer mixed only its
low 32 bits into the hash, so all of them hashed alike:

```js
if (typeof x === "bigint") return $mix(h, Number(BigInt.asUintN(32, x)));
```

The trie can't branch on equal hashes, so they shared one collision list,
which every insert walked and every lookup scanned. Both native backends
truncated the same way, to give the same number.

An `I64`, `U64`, `I128` or `U128` now mixes its fewest two's-complement 32-bit
words, low first, through the same FNV-1a step. Anything that fits an `I32`
is still one word, so it hashes as before, and so do every narrower integer,
`Str`, `Char`, `Bool` and float. Natively the wide ones call
`buri_rt_hash_i64` and its three siblings in `cli/runtime/hash.rs`. On
JavaScript a value a double holds exactly is split with `Math.floor`, without
`BigInt` arithmetic.

The hash stays 32 bits wide on every backend, as `runtime.js` needs.

Visible change: `x.hash()` answers a new number for a wide integer outside
the `I32` range, and a `Map` or `Set` keyed by those iterates in a new order.

The issue's suite, 16,000 keys, at load 4–10:

| | low bits, before | after | above bit 32, before | after |
|---|---:|---:|---:|---:|
| JavaScript, build | 27 ms | 27 ms | 4.1 s | 26 ms |
| JavaScript, 200k lookups | 170 ms | 168 ms | 64 s | 183 ms |
| stencil, build | 4.4 ms | 4.3 ms | 325 ms | 4.2 ms |
| LLVM `--release`, build | 2.0 ms | 2.5 ms | 72 ms | 2.3 ms |
| LLVM `--release`, 200k lookups | 4.0 ms | 4.8 ms | 936 ms | 4.8 ms |

The native times are within this machine's noise. Instructions are steadier
(`native::map_keys`, 8,000 keys, per operation):

| | insert, low bits | after | insert, above bit 32 | after | lookup, low bits | after | lookup, above bit 32 | after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| stencil | 2,938 | 2,943 | 209,461 | 2,957 | 1,061 | 1,061 | 205,607 | 1,078 |
| LLVM | 1,863 | 1,862 | 48,873 | 1,875 | 206 | 208 | 47,442 | 224 |

`native::map_keys` bounds those instructions where the kernel counts them.
`agreement::wide_integers_hash_every_bit` pins the hash values on all three
backends, and `collections/test/map.buri` checks that keys differing only
above bit 32 hash apart, which holds on any host.

A float mixed `ToUint32(Math.trunc(x))`, so `0.1` and `0.2` hashed alike;
§6.65 fixes it.

### 6.65 Floats hash every bit, 2026-10-08

A `core/map` of 16,000 `F64` keys `i / 16000.0` took 2.5 s to build on
JavaScript, against 26 ms for keys `0.0, 1.0, 2.0, ...`. A float hashed as
`ToUint32(Math.trunc(x))`, so every key in `(0, 1)` hashed as `0`, and so did
every multiple of `2^32`:

```js
return $mix(h, Math.trunc(x) || 0);
```

Same collision list as §6.64, and both native backends matched it.

Now a float an `I32` or a `U32` holds hashes as that integer, as before. Any
other float mixes its eight bytes, low first, through the same FNV-1a step:

```js
if ((x | 0) === x || x >>> 0 === x) return $mix(h, x >>> 0);
if (typeof x !== "number") return $mix(h, 0);
if (x !== x) return $mixBytes($mixBytes(h, 0), 0x7ff80000);
$hashF64[0] = x;
return $mixBytes($mixBytes(h, $hashWords[$hashLo]), $hashWords[$hashHi]);
```

- **The integer case is forced.** JavaScript holds an `I32`, a `U32` and a
  float in one `number`, so `$hashInto` can't tell `3` from `3.0`. Nothing
  else needs it: `Hash` promises nothing across types.
- **`==` still pairs with `Hash`.** `-0.0 == 0.0`, and both are the integer
  `0`. Every NaN is `==` every other, so every NaN mixes the quiet NaN's
  bytes, whatever its sign and payload.
- **Bytes, not words.** FNV-1a's multiply only carries bits upward, so a mixed
  word reaches the hash's low bits through its own low bits only, and the trie
  branches on the low bits first. A float's sign, exponent and leading
  mantissa sit at the top of its high word. Mixed as two words, 8,000
  millisecond timestamps shared 8 values in their low 15 bits, and
  `native::map_keys` measured fractional keys 1.5x the cost of whole ones.
  Bytes bring that to 1.05x.
- **`F32`** is widened first, as before, so `1.5f32` and `1.5` hash alike.
  `x.hash()` on an `F32` used to be refused by the stencil backend; it now
  takes the derived path.

Natively all of it is `buri_rt_hash_f64` in `cli/runtime/hash.rs`.

Visible change: `x.hash()` answers a new number for a float that isn't an
integer in `[-2^31, 2^32)`, including NaN and the infinities. A `Map` or `Set`
keyed by those iterates in a new order. Whole floats in that range keep their
hash and their order.

16,000 keys, 200,000 lookups, fewest of five runs at load 1–2. The key is
built in the timed loop, so a float key's time includes making it:

| | `Int` | `Str` | whole `F64` | `i / 16000.0`, before | after | `i * 2^32`, before | after |
|---|---:|---:|---:|---:|---:|---:|---:|
| JavaScript, build | 26 ms | 30 ms | 26 ms | 2.52 s | 31 ms | 2.55 s | 29 ms |
| JavaScript, lookups | 136 ms | 162 ms | 135 ms | 30.7 s | 155 ms | 31.1 s | 150 ms |
| stencil, build | 4.5 ms | 5.9 ms | 4.5 ms | 308 ms | 6.3 ms | 308 ms | 6.2 ms |
| stencil, lookups | 21 ms | 36 ms | 23 ms | 3.89 s | 36 ms | 3.85 s | 36 ms |
| LLVM `--release`, build | 1.8 ms | 3.0 ms | 1.8 ms | 70 ms | 2.9 ms | 69 ms | 2.5 ms |
| LLVM `--release`, lookups | 2.1 ms | 7.7 ms | 2.7 ms | 902 ms | 4.3 ms | 904 ms | 4.3 ms |

`Int`, `Str` and whole `F64` keys cost what they did, within noise; the
columns show the after run. JavaScript was the same with `--release`.

Instructions per operation (`native::map_keys`, 8,000 keys), with the
two-word hash this replaced for comparison:

| | whole | `i / 16000.0`, before | words | bytes | `i * 2^32`, before | bytes | timestamps, words | bytes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| stencil, insert | 2,975 | 225,484 | 4,359 | 3,135 | 225,496 | 3,006 | 4,680 | 3,129 |
| stencil, lookup | 1,097 | 223,970 | 1,620 | 1,183 | 223,983 | 1,120 | 1,705 | 1,177 |
| LLVM, insert | 1,887 | 72,879 | 2,686 | 1,963 | 72,893 | 1,893 | 2,898 | 1,962 |
| LLVM, lookup | 237 | 71,933 | 319 | 263 | 71,946 | 253 | 332 | 262 |

`native::map_keys` bounds fractional, `2^32` and timestamp keys within 1.25x
of whole ones. `agreement::floats_hash_every_bit` pins the hash values on all
three backends. `collections/test/map.buri` checks that fractional keys and
ones that agree mod `2^32` hash apart, and that `±0.0` and every NaN hash
together, which holds on any host.

**Still open:** a `[T]` doesn't hash natively at all (`deriveArrayHash` is a
named gap), so a list of floats is covered on JavaScript only.

### 6.66 `sortBy` uses the order it's given, 2026-10-08

The run-time programs of §6.44 and §6.45, built `--release` and set beside
the same work in Rust at `-O2`, gave one gap far wider than the rest. 24,000
sorts of 2,000 `Int`s:

| Input | Buri | Rust `sort_by` |
|---|---:|---:|
| ascending | 281 ms | 18 ms |
| descending | 332 ms | 21 ms |
| 100 sorts of 100,000 shuffled | 122 ms | 79 ms |

`sortBy` was a bottom-up merge from runs of one, so every input took
`log2 n` full passes, whatever its order. It was a third of the samples of
`a_plists`, the slowest single-threaded program. The other candidates were
narrower: `a_pstrings`' loop retires half the instructions Rust's does, a
2¹⁶-node tree retires 1.36 times Rust's, mostly in macOS's `malloc` and `free`,
and tiny tasks wait on the kernel (§6.44). A sort is in the standard library's
own hands, lowered in one place (`lower/lists.rs`, `list_sort`), and every
backend runs that one loop.

```text
scan          in order: copy it; strictly descending: copy it from the end
runs of 4     insertion, in place
each pass     two runs in order: copy them
              the right run's last before the left run's first: copy them swapped
              otherwise merge, and copy the rest once one side runs out
```

Every step keeps the left element unless the comparator answers `Greater`, so
the sort stays stable. A swap needs the right run's largest strictly before the
left run's smallest, so no tie ever crosses.

`z_sorts`, 100 sorts of 20,000 elements, fewest of five alternating runs at
load 1.3–2.1:

| Input | `Int` before | after | | `Str` before | after | |
|---|---:|---:|---:|---:|---:|---:|
| shuffled | 435 M, 25.0 ms | 332 M, 22.8 ms | −24% | 2,351 M, 155 ms | 2,204 M, 148 ms | −6% |
| ascending | 367 M, 18.6 ms | 34 M, 4.3 ms | −91% | 1,719 M, 81 ms | 293 M, 20 ms | −83% |
| descending | 407 M, 21.4 ms | 38 M, 4.6 ms | −91% | 1,652 M, 85 ms | 304 M, 20 ms | −82% |
| ascending, one more on the end | 367 M, 18.8 ms | 146 M, 10.9 ms | −60% | 1,719 M, 81 ms | 759 M, 47 ms | −56% |
| descending in threes | 407 M, 22.9 ms | 211 M, 14.0 ms | −48% | 1,737 M, 94 ms | 1,238 M, 74 ms | −29% |
| 20 ascending runs | 399 M, 20.4 ms | 224 M, 14.5 ms | −44% | 2,131 M, 114 ms | 1,455 M, 86 ms | −32% |

The run-time programs that sort, same runs:

| Program | Instructions before | after | Wall before | after |
|---|---:|---:|---:|---:|
| `a_plists` | 23,120 M | 16,467 M | 1,064 ms | 765 ms |
| `a_lists` | 2,904 M | 2,072 M | 136 ms | 99 ms |
| `lists`, 100,000 shuffled | 407 M | 359 M | 23.7 ms | 23.1 ms |
| 24,000 sorts of 2,000, ascending | 6,354 M | 457 M | 281 ms | 24 ms |
| the same, descending | 7,167 M | 529 M | 333 ms | 31 ms |
| 100 sorts of 100,000 shuffled | 2,423 M | 1,929 M | 123 ms | 115 ms |

The nineteen that don't sort link to the same bytes. Against Rust that leaves
24 ms to 18, 31 to 21, and 115 to 79.

A debug build gains on order the same way: 86% off the ascending and
descending programs, 46% off `a_lists`. A shuffled list retires 1% fewer
instructions and takes 1–4% longer.

**What it costs to compile.** A `sortBy` call is a loop at its call site, and
the loop is bigger now. `--release` `emit` of a program with one `Int` sort went
from 183 M instructions to 292 M, and a debug build's from 15.5 M to 16.5 M.
The stripped hello world is unchanged at 322,464 bytes.

`agreement::sorting_agrees_on_every_input_shape` sorts ten shapes at nineteen
lengths, up and down, through a lambda and through a function value, on all
three backends under the heap check, against Rust's stable sort.
`native::collection_costs` bounds an ascending and a descending list under a
quarter of a shuffled one. Before, they were 0.83 and 0.94 of it on LLVM, and
0.73 and 0.77 on the copy-and-patch backend. Now they're under a tenth.

A comparator that isn't an order now gets a different permutation than before.
JavaScript's `Array.prototype.sort` already gave a third one.

**Tried and dropped:**

- **Insertion runs of 1, 2, 8 and 16.** On the first draft, 4 was fewest on
  both backends: 147 and 801 instructions an element on a shuffled list,
  against 172 and 858 for no insertion and 164 and 991 for 16.
- **One move loop that wraps at a cut**, for the copied and swapped runs.
  `emit` was 90 M lower per site, but LLVM spilled around the merge's
  comparator call, and shuffled `Str`s retired 11% more instructions than
  before.
- **A copy loop per case.** Three of them cost 200 M of `emit` per site, and
  one shared loop with a second leg costs 109 M.
- **Keeping the merged element across the comparator call.** The
  copy-and-patch backend keeps it in its frame, and a shuffled list took 7–11%
  longer than before. Reading it again costs LLVM nothing: it folds the two
  loads.
- **One back edge for the merge**, through a latch that checks both sides:
  12% slower in debug than two back edges, and 17% more instructions in
  `--release`.

### 6.67 Compiled programs allocate from the compiler's pages, 2026-10-09

The 27 programs of §6.66, profiled again in `--release`. One had a hotspot
the rest didn't share: `b_tree`, 400 builds and drops of a 2¹⁶-node tree,
spent 54% of its samples in `libsystem_malloc`, in `_xzm_free`, its zeroing
`memset` and `_xzm_xzone_malloc`. Every node is two blocks, and the block
cache doesn't hold a tree: a thread's share is 419 KB on ten cores, and a
drain gives a slot back two sweeps after its last pop. So each block was a
`malloc` and a zeroing `free`. In every other single-threaded program the
cache already caught them, and `libsystem_malloc` wasn't a top frame.

The ceiling first. mimalloc through `DYLD_INSERT_LIBRARIES`, set by
`/usr/bin/env` because SIP strips it on the way through `/usr/bin/time`:

| Program | system | mimalloc |
|---|---:|---:|
| `b_tree` | 30,498 M, 1,351 ms | 15,874 M, 773 ms |
| `tree` | 1,550 M, 71.6 ms | 852 M, 45.9 ms |
| five others | | within 1% |

§6.54's allocator already does what mimalloc does here, for the compiler.
So the runtime installs it too:

```rust
// cli/runtime/lib.rs
pub(crate) mod allocator;
#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: allocator::Allocator = allocator::Allocator;
```

- **One source.** `cli/src/allocator.rs` moved to `cli/runtime/allocator.rs`,
  and `cli/src/lib.rs` includes it by `#[path]`. Not under `cfg(test)`,
  because its own tests count pages nothing else may take.
- **A drained program gives pages back.** Each cache sweep calls `trim`, which
  returns empty pages past the pool's 8 MB. `trim` now checks the pool's size
  before taking the lock.
- **`mmap` and `munmap`** are declared with `c_void`, as `memory.rs` declares
  them, or the two clash in `buri-rt-tests`.

`--release`, fewest of five alternating runs, load 1.2–3.7, no sleep in
`pmset -g log`:

| Program | Instructions before | after | Wall before | after |
|---|---:|---:|---:|---:|
| `b_tree` | 30,489 M | 16,898 M | 1,362 ms | 835 ms |
| `tree` | 1,540 M | 861 M | 71.5 ms | 45.3 ms |
| `strs` | 829 M | 741 M | 44.8 ms | 43.5 ms |
| `a_tiny` | 13,213 M | 12,061 M | 1,387 ms | 1,361 ms |
| `a_mapk` | 2,676 M | 2,631 M | 219 ms | 216 ms |
| `a_maps` | 1,513 M | 1,494 M | 100 ms | 99 ms |

The other 21 retire within ±0.5%. `strs` gains from the Rust side of the
runtime, whose own `Vec`s now come from pages too. Against Rust at `-O2`,
`tree` was 1.36 times its instructions and is now 0.76: 861 M and 45 ms
against 1,131 M and 61 ms.

A debug build, fewest of three: `b_tree` and `tree` −35%, `strs` −7.5%,
nothing else past ±1.8%.

Instructions a node, built and dropped until 2²⁰ nodes have been:

| Depth | before, `--release` | after | before, debug | after |
|---|---:|---:|---:|---:|
| 6 | 412 | 412 | 584 | 585 |
| 13 | 1,095 | 601 | 1,230 | 789 |
| 16 | 1,132 | 613 | 1,265 | 801 |
| 18 | 1,152 | 650 | 1,284 | 838 |

Depth 18 is 25 MB of blocks, past the pool's 8 MB, so each drop releases
pages the next build takes back.

**Memory.** Peak footprint is about 140 KB higher in a single-threaded
program, one partly used page per size class, and 1.3–1.6 MB higher in the
fan-out programs, one set per worker. `a_mapk`'s is 18% lower. After dropping
a 2²¹-node tree, about 200 MB of blocks, an idle program's footprint was
83 MB on the system allocator and is 9.6 MB now.

The stripped hello world went from 322,464 to 339,120 bytes in `--release`,
and 356,544 to 373,200 in debug: 5.7 KB of `__text` crossed a 16 KiB page.
The released-page stack's 4 MB is `__bss`.

**Four string programs take 3–8% longer at the same instructions**:
`st_churn`, `a_pstrings`, `a_strings` and `a_pfloats`. It's layout, not the
allocator. Their cache hits never reach it, and a build whose one data byte
sent every allocation to the system, with the paged build's code byte for
byte, was as slow as the paged build. With no allocator change, 16 bytes of
thread-local ahead of the cache moved `a_pstrings` 5.6% and `a_strings` 4.2%,
and aligning every runtime function to 64 bytes moved `a_strings` 5%.
Ordering the runtime's hot functions is the lead if those few percent matter.
§6.68 found otherwise: nine random layouts kept the gap, and turning off the
M3's pointer prefetcher closed it.

`native::collection_costs::a_big_tree_costs_about_what_a_small_one_does_a_node`
bounds a 2¹⁶-node tree under 1.8 times a 63-node one, a node. It was 2.71 on
LLVM and 2.16 on the copy-and-patch backend, and is 1.47 and 1.37. The same
program checks trees built in a fan-out and dropped by the caller, under the
heap check, with no block live at exit.

**Tried and dropped:**

- **Colouring pages**, the first block offset by the page's index times 64,
  in case the hot blocks shared cache sets: no change.
- **Pages for Buri's blocks only**, through `memory.rs`'s four calls: the same
  as the global allocator, less `strs`' 11%.
- **The cache's counters ahead of its slots**: 0.5% fewer instructions in the
  string programs, and wall within noise.

### 6.68 Buri's threads turn off the pointer prefetcher, 2026-10-09

§6.67 left `st_churn`, `a_pstrings`, `a_strings` and `a_pfloats` 3–8% slower
at the same instructions, and blamed code layout. Layout isn't it. Nine
relinks of `a_strings`, each with every text symbol in a random order through
`-order_file`, cycles fewest of five:

| Runtime | nine layouts |
|---|---:|
| before §6.67 | 368–385 M |
| §6.67 | 405–419 M |

The rest of the program held still too:

- **Each call retires the same instructions.** Single-stepped under `lldb`,
  every runtime call in `st_churn`'s loop took the same count on both
  runtimes, `buri_rt_free`'s 67 included.
- **`buri_rt_free` and `buri_rt_alloc` at nine alignments**, through an order
  file: all 3–6% over the old runtime.
- **The thread-local cache moved** across a 16 KiB stretch, by a `malloc`
  interposed for the thread-local block: no change on either runtime.
- **The stack moved**, by environment size, across 4 KiB: no change.
- **The block addresses did move it.** Routing `memory.rs`'s blocks to the
  system allocator, with the pages kept for Rust's own, gave the old cycles
  back. Spreading the size classes over different cache lines, one block a
  line, a 256 MB range and a range without `MAP_NORESERVE` didn't.

That pointed at the M3's data memory-dependent prefetcher, which fetches what
a loaded value looks like it points to. Setting `PSTATE.DIT` turns it off on an
M3 and later. Through an injected library that set it on the main thread,
`st_churn` took 2,589 M cycles on the old runtime and 2,627 M on §6.67's,
against 2,935 M and 3,130 M without it. So the runtime sets it on every
thread that runs Buri code:

```rust
// cli/runtime/host.rs, called by buri_rt_argv_init and rt.rs's thread_loop
pub(crate) fn no_pointer_prefetch() {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if has_dit() {
        unsafe { std::arch::asm!("msr dit, #1", options(nomem, nostack, preserves_flags)) };
    }
}
```

`has_dit` asks `sysctlbyname("hw.optional.arm.FEAT_DIT")` once and keeps the
answer in a zero-initialised static, because `msr dit` is an illegal
instruction on a CPU without the feature. A new thread starts with the bit
clear, which is why `thread_loop` sets it too.
`rt::tests::a_scheduler_thread_runs_with_dit_set` checks both halves where
the CPU has DIT.

`--release`, fewest of seven alternating runs, load 2.9–6.1, no sleep in
`pmset -g log`:

| Program | before §6.67 | §6.67 | now | |
|---|---:|---:|---:|---:|
| `st_churn` | 2,962 M, 790 ms | 3,202 M, 854 ms | 2,608 M, 695 ms | −19% |
| `a_pstrings` | 3,090 M, 824 ms | 3,304 M, 881 ms | 2,768 M, 739 ms | −16% |
| `a_strings` | 373 M, 102 ms | 401 M, 109 ms | 355 M, 97 ms | −11% |
| `strs` | 160 M, 44.6 ms | 154 M, 43.5 ms | 137 M, 38.6 ms | −11% |
| `b_tree` | 5,113 M, 1,360 ms | 3,138 M, 836 ms | 2,987 M, 796 ms | −5% |
| `tree` | 261 M, 71.6 ms | 162 M, 45.2 ms | 155 M, 43.4 ms | −4% |
| `st_build` | 163 M, 46.0 ms | 159 M, 44.9 ms | 155 M, 43.7 ms | −3% |
| `a_pfloats` | 1,242 M, 333 ms | 1,309 M, 351 ms | 1,283 M, 345 ms | −2% |
| `a_pmaps` | 1,384 M, 370 ms | 1,392 M, 373 ms | 1,405 M, 376 ms | +1% |

Cycles, then wall; the last column is cycles against §6.67. `a_pmaps` read
+0.9% to +1.3% on three passes. The other 18 are within ±2.5% at the same
instructions, except `a_tiny`, which waits on the kernel and read +3.8% on
one pass and −3.3% on the next. A debug build moved −7% on `a_pmaps` and
within ±4% elsewhere. The `sysctl` check grew the stripped hello world from
339,120 to 339,184 bytes in `--release`, and 373,200 to 373,248 in debug.

**What DIT costs.** It makes the instructions the Arm spec lists take the same
time for any data. A C loop of 64-bit divides, multiplies, float divides,
pointer chasing and `memcpy` ran no slower with it set.

**Where it does nothing.** Linux compiles the function empty. An M1 or M2's
prefetcher doesn't answer to DIT, by the GoFetch paper's account, so those
Macs see no change. Nobody has measured an M4.

There are no hardware counters to confirm this with. The Command Line Tools
have no `xctrace`, and the configurable counters need root.

The compiler runs on the same allocator (§6.54), and its threads don't set the
bit. §6.69 measured that: it doesn't help the compiler.

**Tried and dropped:**

- **A linker order file for the hot runtime functions**, §6.67's lead. The
  shuffled layouts above put a bound on what any order could win.

### 6.69 The compiler keeps the pointer prefetcher, 2026-10-09

§6.68's lead: `buri` allocates from the same pages as compiled programs, so
might its threads gain from `PSTATE.DIT` too? No. It's 0–2% slower with it,
so the compiler doesn't set it.

The measurement needed no code change. A library injected through
`DYLD_INSERT_LIBRARIES` sets DIT in its constructor and wraps
`pthread_create` so every new thread sets it before it runs:

```c
static void *start(void *p) {
  struct tramp t = *(struct tramp *)p; free(p);
  __asm__ volatile(".inst 0xd503415f");  // msr dit, #1
  return t.f(t.a);
}
```

The baseline injects the same library without the `msr`, so both pay for the
wrapper. Both drop the variable from the environment, so the linker, `bun` and
the test binaries run as usual. A counting copy confirmed it on a large build:
159 of 159 threads ran with DIT set.

A release `buri` built with `--features backend-llvm`, on an M3 Pro with
FEAT_DIT. 22 alternating runs per arm, `buri clean` before each, load 2.3–5.0
on 12 cores, no sleep in `pmset -g log`. "Large" is §6.54's `mixed-100k`
twice, node and native. Fewest of 22, cycles from `/usr/bin/time -l` for the
`buri` process alone:

| Workload | wall, base | DIT | cycles, base | DIT | |
|---|---:|---:|---:|---:|---:|
| large, `build //...` | 0.51 s | 0.52 s | 5.88 G | 5.98 G | +1.6% |
| large, `build --release //...` | 2.52 s | 2.53 s | 74.8 G | 75.0 G | +0.3% |
| large, `lint //...` | 0.24 s | 0.24 s | 0.904 G | 0.907 G | +0.3% |
| example, `build //...` | 0.46 s | 0.46 s | 0.918 G | 0.934 G | +1.8% |
| example, `test //...` | 0.21 s | 0.21 s | 0.471 G | 0.487 G | +3.5% |
| conformance, `test //...`, 4,205 tests | 2.12 s | 2.12 s | 3.23 G | 3.24 G | +0.6% |
| `buri lsp`, 40 edits with a diagnostic pull each | 3.89 s | 3.91 s | 15.30 G | 15.37 G | +0.4% |

Instructions match within 0.2%. Medians agree in sign, except the
conformance run at +0.1%. The example's test, at 0.2 s, is mostly process
start. `BURI_PROFILE` on the large build, 11 runs per arm, found no phase that
gains: lex and parse level, check, middle and emit 0–2% more CPU.

So whatever the prefetcher costs the string programs, it doesn't cost the
compiler. The runtime keeps §6.68's change, and `buri`'s threads leave the
bit alone.

### 6.70 The language server publishes findings in linear time, 2026-10-09

A file with a finding on every function cost the language server time
quadratic in the file. One file of `n` functions, each with a bound its body
never uses, instructions per keystroke (a `didChange` and a pull):

| `n` | before | after |
|---:|---:|---:|
| 1,000 | 1,814 M | 246 M |
| 2,000 | 6,672 M | 491 M |
| 4,000 | 26,058 M | 983 M |

At 4,000 a keystroke took 1.67 s and takes 0.074 s.

**Where the compiler's time goes.** §6.69's workloads, profiled with
`BURI_PROFILE` and `sample`, at load 1–4 on 12 cores:

- `mixed-100k`, `build //...`: 0.52 s. Emission is 2.4 of 7.9 G
  instructions and linking 1.7 G.
- The same with `--release`: 174 of 180 G is LLVM's, under emission.
- `lint //...`: 3.3 G. The rules cost 1.8 G, more than lexing and checking
  together.
- Conformance, `buri test //...`: 2.2 s, of which the compiler is 1.0 s of
  CPU. The rest is the test processes.
- `buri lsp`, a keystroke in `mixed-100k`: 1.3 G. Lint rules 44%, analysis
  30%, `Sources` reading every file 12%, publishing the findings 8%.

None of the cold phases grew faster than its input. Sweeping one file's count
of findings found the language server's publishing did.

**Two scans per finding.** `convert::diagnostic` turned each span into a
position by walking the file from the top, so each finding cost the file's
length. `filed` and `merge_findings` then compared each finding with every one
already in its file's bucket. Both are `O(findings × file)`.

The fix reads positions off the line starts the source map already keeps, and
deduplicates through a set:

```rust
let line = self.starts.partition_point(|s| *s as usize <= offset).saturating_sub(1);
```

```rust
fn dedup_findings(published: &mut Published) {
    for bucket in published.values_mut() {
        let mut seen = std::collections::HashSet::new();
        bucket.retain(|item| seen.insert(finding_key(item)));
    }
}
```

`filed` now only pushes, and a bucket is deduplicated once it's full, keeping
each finding's first copy in the order it was filed. That's what checking at
each push kept. The key is the message and the range written as JSON, which is
equal exactly when the values are: objects write their keys in order, and
neither holds a float.

On `mixed-100k`, 40 keystrokes with a pull each, eight alternating runs per
arm, load 2.2–4.3, no sleep in `pmset -g log`, fewest of eight:

| | before | after | Δ |
|---|---:|---:|---:|
| wall | 3.86 s | 3.74 s | −3.2% |
| cycles | 15.26 G | 14.71 G | −3.6% |
| instructions | 53.08 G | 50.76 G | −4.4% |

**Output is identical.** Whole LSP sessions — opens, keystrokes, pulls, a parse
error, a type error and a `workspace/diagnostic` — are byte-identical on
`mixed-100k`, the conformance repository and both one-file shapes. Nothing
outside the language server changed, and the cold `build //...`,
`build --release //...`, `lint //...` and conformance `test //...` hash the
same before and after.

`build::profile`'s `publishing_a_files_findings_is_linear_in_how_many_it_has`
guards it: an open and a pull over a file of 200 and of 400 findings. Before,
the second cost 3.14 times the first.

**What's left.**

- **`unused-context` is quadratic in the findings in a package** (§6.71 fixes it).
  `context_edits` rebuilds the package's published names, and walks every body
  for call sites, once per finding. One file of `n` functions whose `ctx` is
  never read: `lint //...` is 1.05, 3.7 and 14.8 G at 1,000, 2,000 and 4,000,
  and a keystroke still 1.0, 3.8 and 15.1 G after this change.
- **`Names::of` looks every identifier up in a `BTreeSet<String>`** (§6.71 hashes it). That's
  14% of a `mixed-100k` keystroke in `memcmp`. A set of the module's distinct
  names first would make it a lookup per name rather than per token.

### 6.71 `unused-context` walks the package once, and `Names::of` hashes, 2026-10-09

§6.70's first lead. `context_edits` built the fix for one `unused-context`
finding at a time. For each one it rebuilt the package's published names and
walked every body for the call sites to rewrite, so a file of functions that
never read their `ctx` cost time quadratic in the file.

Now the rule collects its findings first and asks for every fix in one walk:

```rust
typed::ExprKind::CallFn { func: callee, args } => {
    let Some(func) = callee.decl() else { return };
    let Some((index, edits)) = wanted.get_mut(&func) else { return };
    ...
}
```

Each function's edits are pushed in the order the old walk met them, and a
function is refused on the same three grounds: the package publishes it,
something takes it as a value, or a body didn't check.

Cold `lint //...`, process instructions. "Calls" is a binary of `n` private
functions called from one that hands `ctx` on, so every fix rewrites a call
site. "Published" is a library that exports all `n`.

| Shape | `n` = 1,000 | 2,000 | 4,000 |
|---|---:|---:|---:|
| calls, before | 0.81 G | 2.50 G | 6.49 G |
| calls, after | 0.30 G | 0.43 G | 0.66 G |
| published, before | 1.07 G | 3.75 G | 14.85 G |
| published, after | 0.28 G | 0.41 G | 0.66 G |

At 4,000 the published library lints in 0.04 s instead of 0.91 s. A keystroke
in it, after §6.70, went from 1.0, 3.8 and 15.1 G to 0.23, 0.45 and 0.90 G.
`mixed-100k` and the conformance repository have no `unused-context` findings,
and their lint is unchanged at 3.35 and 2.31 G.

**Output is identical.** `lint //...`, `--error-format=json`, `--fix`'s report
and the tree it leaves hash the same before and after on 10 repositories:
`mixed-100k`, the conformance repository, both one-file shapes, and the calls
shape at 200 and 1,000 three ways. The three are fixable, one function in seven
taken as a value, and a type error in the caller. LSP sessions on five of them
are byte-identical too.

`build::profile`'s `unused_context_is_linear_in_its_findings` guards it: the
calls shape at 500 and 1,000. Before, the second cost 3.09 times the first.

**`Names::of` hashes.** §6.70's second lead. `unused-type` and its siblings
look every identifier token in the package up in the set of names written so
far. That set was a `BTreeSet<String>`, so each token cost a dozen string
compares, and a keystroke in `mixed-100k` spent 14% of its time in `memcmp`.
Both sets are now `crate::hash::Set<String>`, and nothing iterates them.

Keystrokes are the fewest of eight alternating runs and lints the fewer of two, at load 1.2–2.4:

| Workload | before | after | Δ |
|---|---:|---:|---:|
| `mixed-100k` keystroke ×40, wall | 3.66 s | 3.22 s | −12% |
| the same, cycles | 14.78 G | 12.88 G | −13% |
| the same, instructions | 50.76 G | 45.22 G | −11% |
| `mixed-100k`, `lint //...` | 3.35 G | 3.08 G | −8% |
| conformance, `lint //...` | 2.31 G | 2.27 G | −2% |

The same ten repositories and five LSP sessions hash identically.

### 6.72 A keystroke reads unchanged modules' names from the kept text, 2026-10-09

Where a `mixed-100k` keystroke's lint goes. `sample` over 150 keystrokes, each
with a pull, on a release build with line tables. Self samples on the thread
that answers, out of 5,420 for the pulls:

| Work | samples | share |
|---|---:|---:|
| lint | 2,214 | 41% |
| analysis | 1,962 | 36% |
| publishing the findings | ~880 | 16% |
| hashing the closure, for the result id | 363 | 7% |

Within lint, `check_hygiene`'s rules are 1,718 samples. The rest are
`dependency_label`'s `stat`s at 138, and the package rules at about 130.

| Rule | samples |
|---|---:|
| `check_unused_declarations` | 491 |
| ↳ `Census::of`, a walk of every body | 258 |
| ↳ `Names::of`, a lookup per identifier token | 205 |
| `check_unused_imports`, a set of every identifier per module | 294 |
| `Held::of`, a second census walk | 238 |
| `check_unused_context_bounds` | 206 |
| `check_unused_variables` | 133 |
| `check_unused_contexts` | 96 |
| `check_discarded_results` | 92 |
| `check_hand_rolled_discards` | 79 |
| `check_deep_nesting` | 63 |

**Two rules read every token of every module on each keystroke, though their
answers depend on nothing but a module's text.** `unused-import` built the set
of names written outside the imports, and `Names::of` asked about every
identifier token. `Text`, which §6.47 keeps per file across keystrokes,
now keeps both answers too:

```rust
/// The names written outside every `import` statement.
used: crate::hash::Set<String>,
/// One token for each name written anywhere but a type's mention of itself.
undeclared: Vec<Span>,
```

A module with a body that didn't check, or a run of source the parser skipped,
still walks its tokens, because what's in doubt depends on the analysis. Every
other module costs a lookup of its text and an insert per distinct name.

`mixed-100k`, 40 keystrokes with a pull each, eight alternating runs per arm,
fewest of eight, load 1.0–1.4, no sleep in `pmset -g log`:

| | before | after | Δ |
|---|---:|---:|---:|
| wall | 3.21 s | 3.03 s | −5.6% |
| cycles | 12.90 G | 12.19 G | −5.5% |
| instructions | 45.22 G | 42.30 G | −6.5% |

A cold `lint //...` builds the sets once per module instead of scanning, and
costs 3.08 → 3.10 G (+0.5%) on `mixed-100k`. Conformance stays at 2.27 G.

**Output is identical** on §6.71's ten repositories: `lint //...`,
`--error-format=json`, `--fix`'s report and the tree it leaves. The five LSP
sessions match too.

No growth guard. `BURI_PROFILE` counts phases, and these rules share a phase
with the analysis a keystroke still runs over the whole target.

**What's left.** Eight rules each walk every typed body: both censuses,
`unused-context`, `unused-context-bound`, `unused-variable`,
`result-discarded`, the hand-rolled discards and nesting. Together that's
about 1,100 samples, a fifth of a pull. The bodies are rebuilt on every
keystroke, so there's nothing stable to key a cache on. Fusing the walks into
one visitor is the lever left, and it touches every rule.

### 6.73 A fan-out wakes one thread, and its joiner runs steps, 2026-10-09

§6.66 left `a_tiny` waiting on the kernel: 20,000 rounds of a 64-step
`tasks.parallel` of `x * 2 + i` took 1.34 s of wall, 1.2 s of user time and
8.6 s of system time. `/usr/bin/time -l` counted 716 K involuntary context
switches, 36 a round. `sample` showed where they came from:

- **The dispatching thread** spent 31% of its samples in
  `pthread_cond_broadcast` and 57% waiting for the latch.
- **22 `buri-thread`s on 12 cores** sat in `__psynch_cvwait` and
  `__psynch_mutexwait`.

`push_all` broadcast `READY` once per fan-out. Every sleeping worker woke and
fought for the run queue's lock. Most found it empty, since 64 steps of one
multiply are gone in microseconds, and went back to sleep. That cost a
syscall and a context switch per thread per round.

The ceiling is the same program on rayon at `-O2`, with
`par_iter().with_max_len(1)` so each item is a job: 599 ms of wall and 5.75 s
of CPU. Rayon pays the kernel too.

The fix has two halves. First, a wake-up goes to one sleeping thread, and only
when nothing else will take the work:

```rust
// cli/runtime/rt.rs, Sched::wake: push_all asks once, take asks after each pop
let wake = !self.queue.is_empty()
    && self.sleeping > 0
    && self.waking == 0
    && self.idle == self.sleeping;
```

`waking` counts wake-ups no thread has come back from. `idle == sleeping`
means no idle thread is awake, since one that has just finished a step is
already on its way to the queue. A backlog wakes the pool one thread after
another, and work that's gone before the next thread arrives wakes no more.

Second, the thread that joins a fan-out runs its own queued steps while it
waits (`help`). It pops from the back while workers take from the front,
switches to each step's stack the way a worker does, and stops at the first
task that isn't one of its own. A task that fans out still parks instead,
because its own stack is the one a step would switch away from. The joiner
also stops while a timer is pending, because timers fire on that thread while
it waits, and a step it ran would hold one back.

Each half alone, behind a switch in a test build, fewest of three to five:

| `a_tiny` | wall | CPU |
|---|---:|---:|
| before | 1,338 ms | 8.64 s |
| joiner runs steps | 1,316 ms | 8.52 s |
| one wake-up at a time | 367 ms | 0.55 s |
| both | 181 ms | 0.21 s |

Six more fan-out programs joined the corpus. Each has 64 steps a round unless
noted: `f_heavy` steps make 4,000 `fromInt`s, `f_mid` 20, `f_uneven` one
step makes 20,000 and the rest 20, `f_nested` is 8 steps of 8 steps,
`f_sleep` is 16 steps that sleep 1 ms, and `f_wide` is 3,000 steps, past the
1,024-step window.

`--release`, fewest of seven alternating runs, load 6–11, no sleep in
`pmset -g log`. Cycles, then wall, then user + system time:

| Program | before | after | wall |
|---|---:|---:|---:|
| `a_tiny` | 26,866 M, 1,345 ms, 8.77 s | 673 M, 183 ms, 0.22 s | −86% |
| `f_mid` | 17,090 M, 808 ms, 5.54 s | 1,870 M, 283 ms, 0.62 s | −65% |
| `f_nested` | 5,355 M, 243 ms, 1.69 s | 1,629 M, 144 ms, 0.50 s | −41% |
| `a_tasks` | 4,063 M, 188 ms, 1.30 s | 1,329 M, 116 ms, 0.41 s | −38% |
| `f_wide` | 33,336 M, 1,598 ms, 10.64 s | 29,147 M, 1,252 ms, 9.25 s | −22% |
| `f_uneven` | 178 M, 24.0 ms, 0.04 s | 90 M, 23.2 ms, 0.02 s | −3% |
| `f_sleep` | 206 M, 550 ms, 0.19 s | 110 M, 539 ms, 0.10 s | −2% |
| `f_heavy` | 1,220 M, 43.9 ms, 0.38 s | 1,171 M, 43.7 ms, 0.36 s | 0% |

The 25 programs that don't fan out are within ±3.5% at the same
instructions. The pool shrank with the wake-ups: `a_tiny` ended with 4 threads
instead of 21, and `a_tasks` with 9 instead of 22. Nothing spins, so a
program that waits burns nothing more; `f_sleep`'s CPU halved. The stripped
hello world is unchanged at 339,184 bytes in `--release` and 373,248 in debug.

Against rayon, fewest of five:

| Program | Buri | rayon | rayon, one thread |
|---|---:|---:|---:|
| `a_tiny` | 183 ms | 599 ms | |
| `a_tasks` | 116 ms | 119 ms | 506 ms |

Buri's `a_tasks` runs one thread's work in 216 ms (`a_seqwork`), so its fan-out
is 1.9 times faster than one thread, where rayon's is 4.3 times. Rayon's
`to_string` is the slower half there.

`fan_out::a_fan_out_of_trivial_steps_wakes_at_most_one_thread` runs 500
fan-outs of `a_tiny`'s steps under the heap check and reads
`buri_rt_tasks_dispatch_wakes` through a linked probe: at most 500, one per
fan-out. The broadcast woke 6,349. It runs on the release backend only,
because the copy-and-patch backend runs a fan-out's steps in order on the
calling thread. `rt::tests::a_batch_wakes_one_thread_and_each_arrival_the_next`
and `a_batch_wakes_nobody_while_an_idle_thread_is_awake` walk `Sched::wake`'s
rule. `the_artifact_says_whether_the_steps_may_fan_out` now tells a fan-out
apart by whether each step ran as a task, since the joiner may run all four
itself. The new test passed 200 runs in a row and 6 × 60 at once, and the
scheduler's 54 unit tests 4 × 30 at once under the heap check at load 50.

**Linux.** The change is `std`'s `Mutex` and `Condvar`, which are futexes
there, and no platform code. It wasn't run on Linux here; CI runs the unit
tests and the release-backend test there.

**Tried and dropped:**

- **Spinning before sleeping**, again (§6.44.1). 1,000, 10,000 and 100,000
  `spin_loop` turns took `a_tiny` from 182 ms to 519, 1,279 and 1,575 ms, and
  `a_tasks` from 121 ms to 124, 234 and 781 ms. Spinning workers took steps
  the joiner runs more cheaply.
- **Two or three wake-ups in flight**: `a_tiny` and `a_tasks` within 1%,
  `f_nested` 148 ms to 160 and 170 ms.
- **A wake-up per pop while work is left**: `a_tasks` 163 ms, `f_mid` 429 ms.
- **The first thread to arrive broadcasts**: `a_tasks` 180 ms, `f_mid` 488 ms.

**What's left:**

- **Task stacks.** 14% of `a_tiny`'s joining thread is `mmap` under
  `new_task`.
- **Nested fan-outs.** A task that fans out parks rather than helping. Helping
  there means switching from one task's stack to another's.
- **Past the window.** A fan-out wider than 1,024 steps joins each step through
  its own `Handoff`, and `f_wide` still takes 9.3 s of CPU for 1.25 s of wall.

### 6.74 A task stack's floor decommit waits 10 ms, 2026-10-09

§6.73 left 14% of `a_tiny`'s joining thread in `mmap` under `new_task`. Task
stacks are already reused: `TASK_POOL` keeps up to 64 released ones, with
their guard pages, and `map_task_stack` takes from it first. A breakpoint on
`mmap` showed what the calls were:

```text
mmap(base + 1 MiB, 63.75 MiB, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANON|MAP_FIXED)
```

That's `buri_rt_task_stack_release`'s decommit, and the stack's watermark was
intact. It was the floor. Every 1,024th release decommits whatever the
watermark says, in case a frame straddled the watermark without writing it.
`a_tiny` releases 1.28 M task stacks, so that's 1,250 re-maps. Each shoots
down every running thread's TLB, about 15 µs and 290 K instructions here.
With the floor off, `a_tiny` retired 361 M fewer instructions and ran 12%
faster.

So the floor still fires on every 1,024th release, but no sooner than 10 ms
after the last one:

```rust
// cli/runtime/memory.rs
fn task_floor() -> bool {
    if TASK_RELEASES.fetch_add(1, Ordering::Relaxed) % STACK_DECOMMIT_EVERY != STACK_DECOMMIT_EVERY - 1 {
        return false;
    }
    let now = crate::host::buri_rt_host_clock_monotonic_nanoseconds();
    let last = TASK_FLOOR_AT.load(Ordering::Relaxed);
    now.saturating_sub(last) >= TASK_FLOOR_NS
        && TASK_FLOOR_AT.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
}
```

The clock is read once per 1,024 releases. A deep task still decommits on
release, as before. The floor only ever caught a stack whose watermark a frame
skipped, and only if that stack happened to be the 1,024th released. At
`a_tiny`'s rate it still fires a hundred times a second. Each thread's own
data stack keeps its per-1,024 floor; it wasn't in the profile.

`--release`, fewest of nine alternating runs, load 8–9, no sleep in
`pmset -g log`. Cycles, then wall, then user + system time:

| Program | before | after | wall |
|---|---:|---:|---:|
| `a_tiny` | 699 M, 167 ms, 0.19 s | 624 M, 141 ms, 0.17 s | −15% |
| `f_mid` | 1,832 M, 286 ms, 0.61 s | 1,809 M, 277 ms, 0.59 s | −3% |
| `a_tasks` | 1,341 M, 125 ms, 0.41 s | 1,317 M, 121 ms, 0.40 s | −3% |

`a_tiny` retires 1,517 M instructions instead of 1,883 M. The other
fan-out programs, and a full corpus pass of seven runs at load 4–10, moved
within the noise of a busy machine. The 25 programs that don't fan out are at
the same instructions. Peak footprint is unchanged: `a_tiny` 6.23 to 6.16 MB,
`f_mid` 6.21 to 6.28 MB. A pooled stack holds what it held before, its
retained 256 KiB and the watermark's page, and the pool's cap of 64 is
unchanged. The stripped hello world is unchanged.

Guard pages aren't touched: the floor decides only whether a released stack
is re-mapped above its guard.
`rt::tests::a_task_machine_stack_is_guarded_at_the_end_it_grows_towards` still
holds. `memory::tests::a_task_stack_decommits_when_deep_and_on_a_floor` sets
the floor's last firing far in the past before its 1,025 shallow releases.
`the_task_floor_fires_once_per_ten_milliseconds` walks the 10 ms on fixed
times.

`fan_out::shallow_steps_decommit_their_stacks_at_most_once_per_ten_milliseconds`
runs 320,000 trivial steps under the heap check. It reads the bytes the
runtime decommitted through the allocation probe, and bounds the re-maps by
one per 10 ms of the run plus one. The bound grows on a slow machine instead
of failing there. Before, the run re-mapped 312 stacks in 179 ms, against a
bound of 18. It passed 200 runs in a row, 6 × 50 at once, and 4 × 25 at once
beside 36 busy loops at load 40.

**Tried and dropped:**

- **Skipping the decommit when a task's Buri data stacks go back to the
  pool**, whose drop re-maps every block whatever its watermark says. No
  change: release-built steps never take a data stack.
- **`mincore` to ask whether anything past the watermark is resident**: 900 µs
  for the 63.75 MiB range on macOS, sixty times a re-map.

### 6.75 A wide fan-out maps stacks for the steps that run, 2026-10-09

`f_wide` runs 100 fan-outs of 3,000 trivial steps, past the 1,024-step window.
It took 1.26 s of wall and 9.2 s of CPU. `sample` found two costs:

- **`__munmap`, 4,481 samples on the workers.** `new_task` mapped every
  step's machine stack when the step was queued. So 1,024 queued steps held
  1,024 stacks, while a dozen ran. The pool keeps 64, so most finished stacks
  were unmapped and most new steps mapped fresh ones: a `mmap`, an `mprotect`
  and two trimming `munmap`s each.
- **The join, a step at a time.** Past the window, each step was a `Handoff`,
  joined in turn through `park_on`, and each join freed one slot, so the
  window refilled with a lock and a wake-up per step. Half the dispatching
  thread's samples were `__psynch_mutexwait` on the run queue's lock.

Three changes in `cli/runtime/rt.rs`:

- **A step maps its stack on its first turn** (`turn`, `give_stack`) when its
  fan-out is wider than the 64 stacks the pool keeps. A narrower one still
  maps up front. Mapping later in `a_tiny` moved the work past the wake-up,
  and the dispatcher ran its steps late enough that more workers joined:
  160 ms to 225.
- **One latch for every step.** The window refills 64 steps at a time
  (`REFILL`) through one wait (`Arrived`, which is ready once the count
  falls to a mark):

  ```rust
  let batch = REFILL.min(n - started);
  park_on(Arrived { latch: &latch, left: n - started + IN_FLIGHT - batch });
  ```

  A step whose body didn't return marks the latch, and the fan-out asserts
  on that at the end, as each join used to.
- **The dispatcher keeps the steps it made** until they finish, and drops
  them itself. Dropped by whichever thread finished them, they were freed
  across threads, and `a_tiny` went from 160 ms to 250.

The window now slides in steps of 64. The 1,025th step starts once 64 of the
first 1,024 have finished, rather than one. The answer and its order don't
change.

`--release`, fewest of nine alternating runs, load 2–6, no sleep in
`pmset -g log`. Cycles, then wall, then user + system time:

| Program | before | after | wall |
|---|---:|---:|---:|
| `f_wide` | 29,064 M, 1,261 ms, 9.23 s | 6,038 M, 216 ms, 1.93 s | −83% |
| `f_mid` | 1,793 M, 268 ms, 0.59 s | 1,773 M, 266 ms, 0.57 s | −1% |
| `a_tiny` | 593 M, 158 ms, 0.18 s | 601 M, 160 ms, 0.19 s | +1.5% |
| `a_tasks` | 1,340 M, 115 ms, 0.42 s | 1,343 M, 115 ms, 0.42 s | 0% |

`f_wide` retires 3.6 G instructions instead of 35.6 G, and its peak footprint
went from 57.0 MB to 6.5 MB. `a_tiny` retires 2% more instructions: the
latch's mark and the stack counters. A full pass of seven runs, at load 4–11,
had the programs that don't fan out at the same instructions. The stripped
hello world is unchanged.

`fan_out::a_wide_fan_out_refills_in_batches_and_maps_stacks_for_running_steps`
runs 20 fan-outs of 3,000 trivial steps under the heap check. It reads two new
probes, `buri_rt_tasks_batches` and `buri_rt_task_stacks_peak`:

- at most 32 batches a fan-out, which is 1 + ⌈1,976 / 64⌉;
- at most 257 task stacks out at once, the scheduler's 256 threads and the
  caller, since these steps never park.

Before, it queued a batch per step and failed. `rt::tests::a_latch_wakes_its_waiter_at_the_mark`
walks the mark: one wake-up when the count reaches it, none after, and a
failed step remembered. The new test passed 150 runs in a row, 6 × 30 at once,
and 4 × 20 beside 36 busy loops. The runtime tests passed 15 full in-process
runs under the heap check.

**Tried and dropped:**

- **Mapping every step's stack on its first turn**, narrow fan-outs too:
  `a_tiny` 160 ms to 225.
- **One idle stack per thread, ahead of the pool's lock**: `f_wide` 216 ms to
  160 and 1.93 s of CPU to 1.14, but `a_tiny` 4% slower. On macOS each
  access is a `tlv_get_addr` call, two per task.

### 6.76 Where a `buri test` run's time goes, and suite keys side by side, 2026-10-09

A release `buri` from `247a5fa8c` with `--features backend-llvm`, on a 12-core
M3 Pro at load 2–4 unless noted, no sleep in `pmset -g log`. The phases come
from `BURI_PROFILE` and a local probe, never committed, that wrote a timestamp
at every phase change, process start and runner's first line. The workloads:

- **conformance**: `cli/tests/conformance` with `backends: [JS]` dropped where
  stencil compiles the suite. 33 native suites in one runner, 7 on `bun`,
  4,205 tests.
- **conformance ×10**: every package copied under ten names, 390 suites.
- **example**: `cli/tests/example`, 8 suites, 6 native in two runners.
- **edit**: rewrite one test in `//lib/calendar/test/date.buri`, then
  `buri test //...`. For the example, change a constant in `//lib/money`, which
  five suites read.

**An edit is mostly the launch check.** Conformance, edit, 0.33 s:

| Step | Wall |
|---|---:|
| start, open the workspace | 3–6 ms |
| key and look up all 40 suites | 19–25 ms |
| check, lower and emit `//lib/calendar` | ~20 ms |
| link its 1 MB runner (`ld64.lld`) | 40 ms |
| §6.39's launch check of that runner | 228–240 ms |
| 12 runner processes, 117 tests | 8–10 ms |

**A cold run waits on generators, one thread and the check.** Conformance
after `buri clean`, 1.39 s:

| Step | Wall |
|---|---:|
| compile the generator tool, 20 `bun` runs | 254 ms |
| key and look up all suites | 36 ms |
| check, monomorphize and middle end of the one batch program, one thread | ~300 ms |
| emit, 12 threads | 40 ms |
| stage 180 objects, `ld64.lld` | 130 ms |
| launch check of the 27 MB runner | 436 ms |
| 72 runner processes, 4,205 tests | 190 ms |

Every other workload, by its largest costs:

| Workload | Wall | Largest costs |
|---|---:|---|
| conformance, nothing changed | 24 ms | keys 19 ms |
| conformance ×10, nothing changed | 0.31 s | keys 0.30 s |
| conformance ×10, edit | 0.60 s | keys 0.28 s, launch check ~0.23 s |
| example, cold | 0.21–0.31 s | two launch checks, `bun` |
| example, edit `//lib/money` | 0.29 s | link 44 ms, compile 40 ms, two launch checks |
| conformance `--release`, edit | 0.50 s | LLVM 205 ms on one thread, launch check, link 43 ms |
| conformance `--release`, cold | 8.7 s | LLVM 80.6 s of CPU on 12 threads, 86% of wall |

**The launch check costs about 0.2 s plus 10 ms per MB on a quiet machine.**
A fresh copy of a 1 MB runner took 0.13–0.26 s to start, and of a 27 MB one
0.40–0.47 s. At load 15–35, with other agents' tests in the same queue, either
size took 4–17 s, and `XprotectService` held 50–60% of a core all day. §6.63's
program store doesn't help here: an edit makes new bytes, so the runner is a
new file whatever the store holds. That leaves the next cost.

**Suite keys were the next, and the only one that grows with the
repository.** Every `buri test //...` keys every suite before it looks
anything up. A key reads the suite's test sources, lexes them for the
comment-blind program text (§6.43) and hashes them, and `plan` did that one
suite at a time. `sample` of the conformance keys: lexing 45%, SHA-256 17%,
copies 12%, `open`/`read`/`stat` 14%. Conformance ×10 spent 0.30 s there on
every run, half of an edit.

No key reads another's result, so `commands/test.rs` now takes them on all
cores before the lookups:

```rust
let keys = crate::parallel::map(runs.len(), |r| {
    let &(i, platform) = runs.get(r)?;
    let output = crate::build::buildfile::Output::for_platform(platform, Span::NONE);
    Some(actions::test_key(session, *suites.get(i)?, &output, flags))
});
```

`runs_of` is the platform choice `plan` made, factored out so both read one
list. The lookups, the build keys of suites that miss, and everything after
them run as before.

Five alternating runs per arm, median wall, load 2.9–3.2:

| Workload | Before | After | Δ |
|---|---:|---:|---:|
| conformance, nothing changed | 26 ms | 14 ms | −46% |
| conformance, edit | 0.328 s | 0.316 s | −4% |
| conformance ×10, nothing changed | 0.312 s | 0.163 s | −48% |
| conformance ×10, edit | 0.595 s | 0.438 s | −26% |
| example, cold, best of three | 0.214 s | 0.190 s | −11% |

Instructions rise about 1%, the threads' cost. Cold conformance didn't move
(1.30 s and 1.29 s best of three): its keys are 36 ms of 1.39 s.

**Output is identical.** A build that also computed every key serially and
aborted on a mismatch ran cold, `--output=js` and warm over all 395 fixture
repositories under `cli/tests`, the example, the tutorial and conformance:
no mismatch. Base and new printed the same `--verbose` listings, summaries
and exit codes for the same three runs of every one of them, with times
masked.

There's no growth guard. The work is the same, only spread over threads, and
no count tells that apart from serial.

**What's left, largest first:**

- **The launch check**, 70% of a quiet edit and seconds under load. Only fewer
  new executables avoid it. Loading a suite's code into a checked runner was
  declined. On a developer's own machine, `spctl developer-mode
  enable-terminal` and adding the terminal under Privacy & Security →
  Developer Tools skip it, about 0.23 s per edit. Smaller stencil code would
  shorten the cold run's 27 MB check, at about 10 ms per MB.
- **One module's LLVM codegen runs on one thread.** The `--release` edit
  spends 205 ms in `date.buri`'s unit. Splitting the module after optimization
  and generating its machine code in parallel could save ~0.15 s per release
  edit. A cold release is already 12-way parallel.
- **The link**, 40–45 ms: `ld64.lld` starts in 143 M instructions, and loading
  the 19 MB runtime archive's 78 members takes 22 ms. A runtime linked ahead
  of time saves ~10 ms at most (§6.77).
  Its debug sections aren't the cost: stripped, the link retired the same
  instructions.
- **Cold, before codegen:** every suite waits on every generator (254 ms here),
  and the batch program's check, monomorphize and middle end run on one thread
  (~300 ms, middle 144 ms of it).
- **Lookups stay serial**, 51 µs per suite, and so does the build key of each
  suite that misses, 0.6 ms each: 20 ms and 44 ms at 390 suites.
- **Workers poll every millisecond** while the first runner waits on the check
  (`run_member`), about 3% of a core each. That's CPU, not wall.
- **The first edit to a suite after a full run** re-emits its standard library
  units, since a suite built alone is a different program from the batch.
  Release spends 9.5 G instructions there instead of 2.8 G, on other threads,
  so the wall barely moves.

### 6.77 A runtime linked ahead of time, measured and left, 2026-10-09

§6.76's third lever. Every `buri test` link hands `ld64.lld` the 19 MB runtime
archive, and lld spends 22 ms loading the 78 members a runner uses. Two ways
to link the runtime once, tried by hand on the conformance edit's link
(`//lib/calendar`, 1 MB runner), three to four runs each:

| Runtime as | `ld64.lld` instructions | Wall | Runner |
|---|---:|---:|---:|
| the archive, as today | 542–548 M | 40 ms | 1.06 MB |
| one object, `ld -r -all_load` | 639–649 M | 40–50 ms | 9.8 MB |
| the same, debug sections stripped | 641–645 M | 40–50 ms | 9.8 MB |
| a dylib, `-all_load` | 410–414 M | 30 ms | 0.52 MB |

- **One object is worse.** Apple's `ld -r` merges the members into one
  section per kind, so `-dead_strip` keeps the whole runtime. `ld64.lld` has
  no `-r` at all.
- **A dylib saves ~10 ms a link and nothing else.** Building it takes 0.13 s
  and 694 M instructions, once per runtime. Each runner process then binds it
  at start: 26.7 M instructions against 20.4 M, at the same 8.6 ms wall over
  40 launches. A conformance edit starts 12 of them, so the processes give back
  about half the instructions the link saves. The smaller runner doesn't shorten the
  launch check: fresh 0.52 MB and 1.06 MB runners both took 0.20–0.22 s.

That's ~10 ms of a 0.32 s edit, about 3%. In return, test runners would link
differently from `buri build`'s binaries: a dylib kept by its bytes in
`BURI_HOME`, an `rpath`, calls into the runtime through stubs, and one launch
check of a 9.5 MB library per runtime. That isn't worth 3%, so the link is
unchanged.

`ld64.lld` itself is now most of the link: 143 M of the 542 M instructions are
its start-up, before it reads an input.

### 6.78 Release machine code on several threads, measured and left, 2026-10-09

§6.76's lead: a `--release` edit of `//lib/calendar/test/date.buri` spends
about 0.2 s in LLVM on one thread. On a quiet machine, the edited unit's time
splits about evenly:

| Step | Wall |
|---|---:|
| build the unit's IR | 8 ms |
| `default<O2>` | 90–100 ms |
| machine code | 120–130 ms |

The edit re-emits about 30 units side by side. The longest chain was
`//lib/bignum/test/bigint.buri`'s, at 112 ms of `opt` and 175 ms of machine
code. Splitting can only take the machine-code half, since `opt` runs before
the split so inlining sees the whole module. Two ways of dividing it were
built and measured:

```text
optimized module ──bitcode──▶ part 0 … part n, each in an LLVM context of its own:
                                its functions defined, the rest declared,
                                its constants available_externally so loads still fold
                              ──▶ one object per part, on parallel::map's threads
```

**One assembled object: slower than one thread.** Each part printed its
assembly, its local labels were renamed apart, and the text was assembled
once as a carrier module's inline asm, so the unit stayed one object.
Assembling is the cost. `bigint`'s 317 KB of assembly took 110 ms to
assemble, after 185 ms for two parts, against 175 ms for the whole unit in
one go.

**An archive of objects: faster, but not neutral.** Each part became its own
object, packed with an empty table of contents into an archive and linked
with `-force_load`. Both `ld64.lld` and Apple's `ld` accept such an archive.
Local symbols that another part names became hidden and got a per-unit
suffix. The conformance copy passed release-built, 3,601 tests, the same as
without splitting. One-edit runs, fewest of nine alternating, load 4–8; cold
runs, fewest of three, load 6–34:

| Part size, IR instructions | edit | CPU | cold | CPU |
|---|---:|---:|---:|---:|
| no split | 574 ms | 1.63 s | 10.80 s | 92.9 s |
| 3,000 | 573 ms | 1.69 s | 10.37 s | 101.1 s |
| 1,500 | 480–504 ms | 1.85–1.90 s | 10.58 s | 102.9 s |
| 600 | 503 ms | 2.09 s | | |

So the gain is about 13% of an edit and a few percent of a cold run, for
10% more CPU. The rest of an edit is the launch check, the link and `opt`.
A cold run already keeps every core busy.

**What stopped it: parts don't make the code the whole module made.** With
every corpus program forced into parts, outputs matched, but `s_rand`
retired 0.7% more instructions, `z_sorts` and `lists` 0.6% and `b_lists`
0.4%. The other single-threaded programs stayed within 0.1%. `s_rand`'s hot
loop took four more instructions. The split isn't the cause: `llc` on the whole module's bitcode
writes the same loop as the part does. The bitcode round trip is, most
likely through use-list order, which `LLVMWriteBitcodeToMemoryBuffer`
doesn't keep and which passes such as loop strength reduction iterate.
Release output comes first, so the split wasn't kept.

What would make it neutral: write each part's bitcode with use-list order
preserved (`WriteBitcodeToFile(M, OS, /*ShouldPreserveUseListOrder=*/true)`,
which the C API doesn't expose and would need a small C++ shim), or hand
parts over without serializing at all. Then the archive route gives the 13%
above with identical code.

### 6.79 A cold `buri test`: what can overlap, and a link that reads the cache in place, 2026-10-09

§6.76's fourth lever. Cold conformance, `buri clean` first, 1.36 s at load 6,
from a local timestamp probe:

| Step | Starts | Wall |
|---|---:|---:|
| compile the proto tool | 3 ms | 45 ms |
| 6 `bun` processes run it, side by side | 48 ms | 125–130 ms |
| key and look up all suites | 183 ms | 23 ms |
| parse the batch program | 212 ms | 18 ms |
| check it, one thread | 230 ms | 59 ms |
| monomorphize, one thread | 289 ms | 39 ms |
| middle end, one thread | 340 ms | 131 ms |
| lower (already per function) and key the units | 486 ms | 26 ms |
| emit, 12 threads | 492 ms | 37 ms |
| write 180 objects to the cache | 529 ms | 31 ms |
| hard-link them into the link directory, 12 threads | 568 ms | 46 ms |
| `ld64.lld` | 616 ms | 90 ms |
| launch check, then the tests | 711 ms | ~620 ms |

The middle end, pass by pass: `inline` 37 ms, `rc::analyze` 33 (already per
function inside), `derives` 15, `chunks` 7.6, `closures` 6.3, `fuse` 5.8, `dce`
5.1, `forward` 4.7, `decision` 4.4, `rc::Syntactic::new` 4.3,
`rc::name_discards` 3.9, `tail_calls` 3.6.

**Measured and left:**

- **Starting suites before the generators finish.** Only the two proto suites
  read generated code, but the rest share their batch program and its runner.
  A second batch would start ~185 ms sooner and cost a second runner, whose
  launch check queues behind the first one system-wide. That's 0.2 s at
  least, and seconds under load. Keys alone could overlap, 23 ms at most.
- **The generators themselves.** Loading the tool's 485 KB bundle takes 20–30
  ms of each `bun` process. The rest is the tool's own work.
- **Writing the cache entries side by side.** The store took 30–37 ms against
  30–53 ms, alternating, three each: the file system is the limit, as §6.43
  found for staging.
- **Per-function middle passes.** `fold_round`, `fuse`, `forward` and
  `decision` touch one body each and would parallelize, for ~17 ms. `inline`
  doesn't: a caller reads callees this round already rewrote, so another order
  gives other code. Checking bodies in parallel needs the checker's shared
  tables split, which is a redesign.

**What changed: the linker reads the cache's objects where they are.** A link
hard-linked each object from `.buri/cache` into its directory, and the
linker read the copies. Each hard link costs ~0.37 ms on APFS (180 serially
took 68 ms) and more when twelve threads share one directory: the link
phase's own CPU was 0.40–0.52 s for 180 objects. Now an object the cache holds
is passed to the linker by its absolute path. Only objects the cache doesn't
hold, and the runtime archive, are written into the directory.

The runtime archive stays staged under its relative name because it's the
one input with debug information. It's what the runner's 18 `N_OSO` stabs
name, and `-oso_prefix .` keeps those paths machine-independent (§6.24). The
backends write no debug information, so no stab names a Buri object. The
debug map is the same bytes, and evicting a cache entry or `buri clean`
changes nothing a debugger reads. `lldb` resolves Buri functions by symbol,
as before, and has no line table for them on either side. For the record,
`ld.lld` (ELF) gave identical outputs for absolute and relative inputs too,
with and without `-g`. Linux links name no `--whole-archive` or rpath; the
musl sysroot and crt objects are still staged.

**Identical output.** Base and new linked byte-identical conformance runners,
stencil and `--release`, and identical example artifacts, debug and release,
and printed the same results.
`native::link::a_link_reads_the_caches_objects_where_they_are` links one
program twice, from staged copies and from the cache. It asserts the same
bytes and no object in the link directory, and it fails on the old code.
`two_checkouts_of_one_tree_build_identical_bytes` catches the day a backend
adds debug information and a cache path reaches a stab.

Cold `buri test //...`, alternating, base `01600a72f`, load 4–7:

| Workload | Before | After |
|---|---:|---:|
| conformance, six each, median wall | 1.332 s | 1.271 s (−4.6%) |
| the same, `link` phase CPU in `buri` | 0.40–0.52 s | 0.006–0.008 s |
| conformance ×10, three each, median wall | 7.03 s | 6.25 s (−11%) |
| the same, `link` phase CPU | 1.6–2.2 s | 0.02 s |
| conformance, one-test edit, six each | 0.31–0.38 s | 0.30–0.50 s, within noise |

An edit already linked mostly from the last link's directory, so it doesn't
move.

**What's left** is the one-thread chain, check through middle, ~230 ms of a
cold run, and the launch check.

### 6.80 `buri test`'s work, counted and pinned, 2026-10-09

§6.76 and §6.79 put an edit-and-rerun's time in the launch check of each new
runner, then the link. No instruction count sees either, so a run now counts
the operations:

```text
$ BURI_PROFILE=1 buri test //...      # after editing one test
suites built 1
suites restored 0
suites reused 2
objects compiled 2
objects restored 4
links 1
new executables launched 1
test processes 1
```

- **suites built, restored, reused**: `--explain`'s `run build`,
  `cached build` and `cached test` lines.
- **objects compiled, restored**: its `run codegen` and `cached codegen` lines.
- **links**: linker runs (`CDriver::link`).
- **new executables launched**: first starts of a file `place_from` wrote as a
  new inode, which is what macOS checks.
- **test processes**: native runners and JavaScript runtimes started for tests.

Load and thread scheduling can't move any of them. `build::work_counts` pins
them, plus the files a run wrote (new inodes or changed ones under the
repository), on three suites where two share a library:

| Scenario | Stencil, LLVM `--release` | JavaScript, `--release` too |
|---|---|---|
| cold `//...` | 3 built, 10 objects, 1 link, 1 new exe, 3 processes | 3 built, 3 processes, 7 files |
| rerun | 3 reused, nothing else | the same |
| edit one test | 1 built, 2 objects (4 restored), 1 link, 1 new exe, 1 process, 7 files | 1 built, 1 process, 2 files |
| edit the shared library | 2 built, 3 objects (5 restored), 1 link, 1 new exe, 2 processes, 10 files | 2 built, 2 processes, 4 files |
| `--filter` matching one suite | 3 built, 4 objects (6 restored), 1 link, 1 new exe, 1 process, 10 files | 3 built, 3 processes, 9 files |
| the same `--filter` again | 3 restored, 1 process | 3 restored, 3 processes, 3 files |
| one edited suite named alone | 1 built, 1 object (4 restored), 1 link, 1 new exe, 1 process, 6 files | 1 built, 1 process, 2 files |
| that suite alone again | 1 reused, nothing else | the same |

A cold native run's files written aren't pinned: Linux also writes the musl
sysroot into the cache. Test processes are exact because each suite holds one
test. A suite with more blocks starts helper processes only while blocks are
still waiting, which does depend on scheduling.

**Leads, measured and left:**

- **A runner's members decide some units' code.** The first edit after a cold
  run links `{a}` where the cold run linked `{a, b, c}`, and recompiles
  `core_testing_assert` with the test. A second edit to the same test
  recompiles only the test's unit.
- **An edit that leaves a library's object unchanged still recompiles its
  dependents.** Withdrawn in §6.81: the library's body is inlined into them.
- **`--filter` builds every suite.** Two of the three hold no matching test,
  yet each is compiled into the runner. On JavaScript each also gets a
  process.
- **JavaScript rewrites a restored bundle.** The same `--filter` again writes
  all three bundles, though their bytes match. Native `place_from` leaves an
  identical runner alone.

### 6.81 §6.80's leads, 2026-10-09

The `validate` profile, stencil, on a 12-core M3 Pro at load 11–20. Each
figure is five edit-and-reruns, each giving a test in
`//lib/calendar/test/date.buri` a new name. Conformance is
`cli/tests/conformance`, 39 suites; ×10 copies every package under ten names,
390 suites.

**An unchanged library object doesn't mean unchanged dependents.** The lead
was wrong. Every function in §6.80's fixture is inlined into the test that
calls it, so the library's own object is a stub that never moves, and an edit
to its body really does change both tests' objects. A comment edit reuses
every verdict. An edit to a recursive helper, which isn't inlined, recompiles
only the library's object and the runner's entry (the third lead).

**`--filter` no longer builds a suite it leaves no test in.** The plan reads
the test names off the suite's parsed sources. When none matches, the suite is
answered with its tests listed as skipped, before anything is checked,
linked or started. A source that doesn't parse is built as before, so its
error is still reported. A library that doesn't check, in a suite the filter
leaves nothing in, is skipped rather than reported.

| `--filter=leap year`, after an edit | Before | After |
|---|---:|---:|
| conformance, wall | 0.31–0.38 s | 0.11 s |
| conformance, suites restored, test processes | 38, 39 | 0, 1 |
| ×10, wall | 1.77–2.05 s | 0.33–0.45 s |
| ×10, suites restored, test processes | 389, 380 | 9, 10 |

`--verbose` lists the skipped tests as before, and a suite left with none
shows no build time, since it wasn't built.

**The first edit's extra objects, measured and left.** Conformance, after a
cold run, with `backends: [JS]` dropped from the 32 suites stencil compiles:

| Edit after a cold run | Wall | Objects compiled | `emit` | `run` |
|---|---:|---:|---:|---:|
| first | 0.26–0.27 s | 15 | 11 ms | 182 ms |
| second | 0.11–0.21 s | 1 | 3 ms | 3 ms |

The edited suite's runner holds one suite where the cold run's held 32.
Different instantiations, and an inliner that pastes a body called once
(`assert.equal` from one test, rather than from 32), change 14 library objects.
Reusing them would save 8 ms of `emit`. The rest of the gap is the launch
check of a runner with new bytes, which a first edit always pays, so this is
left.

**A rerun leaves a bundle that holds the same bytes alone.** `write_bundle`
compared nothing and rewrote every bundle it ran, while native `place_from`
skips an identical runner. The same `--filter=JSON` run again now writes 0
files where it wrote 2 on conformance and 20 on ×10. Wall time doesn't move:
0.084 s and 0.33–0.37 s, either way.

**A failing suite's runner stays put while another suite is edited.** A
failing verdict isn't cached, so every pass starts the failing suite's runner
again from its build record. That restore always claimed the shared
`test-runner` file, and an edit's new link claimed it too, first come first
served. Whichever lost was written to its own path, and the next pass moved it
back: a new file, so another launch check, of a batch runner that can be large.
×10 has ten failing suites (its copies of `proto_gen` fail), so an edit there
paid it on five passes of six, at random.

Now a restored runner stays at its own path when that already holds its bytes.
Otherwise it claims the shared file before any link of the pass does, and
keeps it for the whole pass, so no link writes over it. Each pass after the
first edit, three each:

| ×10, edit after edit | Before | After |
|---|---:|---:|
| wall | 0.24–1.27 s | 0.24–0.25 s |
| new executables launched | 0 or 2 | 0 |

The first edit after a cold run still launches two. The cold run left the
failing suite's runner at its group's first member's path, and the restore
looks at the first failing member's.
`work_counts::a_failing_suites_runner_stays_put_while_another_suite_is_edited`
pins one launch per edit, which varied between one and two before.

### 6.82 The goals, gated on CI, 2026-10-09

```text
cargo bench -p buri --bench compiler -- --goals=count   # instructions a line, against a budget
cargo bench -p buri --bench compiler -- --goals=wall    # lines a second, against the goal
```

Two CI jobs hold the three goals, both on the pinned `mixed` corpora:

| Job | Runs | Fails when | Blocks |
|---|---|---|---|
| `goals-counted` | `--goals=count` under cachegrind, arm64 Linux | a phase retires more instructions a line than its budget | always: counts don't move with load |
| `goals-timed` | `--goals=wall` on the 12-core macOS runner | a phase runs below its goal, fastest of several runs | check and dev compile; parse only warns |

The gate checks absolute goals. §9's gate checks growth against a baseline.

**What each phase is:**

| Phase | Counted | Timed |
|---|---|---|
| parse | `parser::parse` over every module of `mixed-1M` | the same, fastest of at least 5 |
| check | `Checker::resume` + `run` over `mixed-1M`, loaded beforehand | the same |
| dev compile | the `buri` process of a cold `buri build` of `mixed-100k` | the wall time of a cold `buri build` of `mixed-1M`, fastest of 3 |

Parse and check are §4's seams, so `--goals=wall` reads what the `lex+parse` and
`sema` rows read. Dev compile is the real command: front end, middle, stencil
codegen and link, to a native binary, which is what a cold `buri test` builds
too. It counts 100k because cachegrind would take minutes over a 1M build, and
100k costs slightly more a line than 1M (36,184 against 35,000). The linker is a
separate process, so the count leaves it out and the wall time has it.

**A budget is the goal at a reference core's instruction rate.** The reference
is one performance core of the M3 Pro these goals were set on (§6), the same
class as the macOS runner's M4:

```text
budget (instructions a line) = rate (instructions a second, one core) / goal (lines a second)
```

The rate is each phase's own, measured as instructions a line times its fastest
single-core lines a second, at load 3, then rounded down:

| Phase | Instructions a line | Fastest on one core | Measured rate | Rate used | Budget | Headroom |
|---|---:|---:|---:|---:|---:|---:|
| parse | 1,675 | 11.81 M lines/s | 19.8 G/s | 18 G/s | 1,800 | 1.07x |

Parse's row is from before §6.83. Since then it's 1,390 a line and 1.29x, and
its budget stays at the goal: a ratchet that blocks any regression past it.
| check | 3,393 | 4.19 M lines/s | 14.2 G/s | 12 G/s | 12,000 | 3.54x |
| dev compile | 36,184 | 251 k lines/s | 8.8 G/s | 8.5 G/s | 85,000 | 2.35x |

Dev compile's single-core figure is lines over the whole process's CPU time,
4.03 s for `mixed-1M`, and its rate is that process's instructions over it.

**Counts are per core, wall times are the whole machine.** Parse and check run
on one thread per program, so for one build the two are the same. A cold build
of `mixed-1M` uses 4.03 s of CPU in 2.62 s, 1.54x parallel on 12 cores, so a
per-core budget only errs on the strict side. The wall time is what you wait
for, so it's taken whole, on a runner with the same core count as this machine.

**The slowest runner isn't the reference.** Its Ampere-1a cores are about a
third of an M3's: PassMark single-thread 1,350 against the x86_64 runner's
4,357, and the arm64 test leg measured 2.6x slower than x86_64. At its rate
every budget is a third of the above, and parse and dev compile would fail
today. They'd be right to: on that machine the goals don't hold. The goals
describe a developer's machine.

**Measured on this M3 Pro**, mixed-1M, single core except dev compile's wall:

| Phase | Goal | Measured | Headroom | `wall` on a miss |
|---|---:|---:|---:|---|
| parse | 10 M lines/s | 11.81 M, 12.71 M after §6.83 | 1.18x, 1.27x after | warns |
| check | 1 M lines/s | 4.19 M | 4.19x | fails |
| dev compile | 100 k lines/s | 394 k whole machine, 251 k a core | 3.93x | fails |

At load 3. Under load 11 they read 1.13x, 3.95x and 3.57x. The other shapes
agree, with `generic-blowup` the worst at every phase: parse 10.9 M, check
3.98 M and lowering 455 k lines/s, and 41,800 instructions a line through a cold
build.

Parse only warns because its headroom is inside run-to-run noise on a loaded
machine, which took 1.18x to 1.05x once at load 37. Its count still blocks.

**CI's first run**, after §6.83, both jobs green:

| Job | parse | check | dev compile | Took |
|---|---:|---:|---:|---:|
| `goals-counted`, instructions a line | 1,373, 1.31x | 2,998, 4.00x | 24,594, 3.46x | 5.5 min, 3.9 of it building |
| `goals-timed`, lines a second | 12.12 M, 1.21x | 3.67 M, 3.67x | 445 k, 4.45x | 2.4 min |

Cachegrind's counts sit below macOS's, most of all for a build, since they
leave out the kernel's work.

**Estimated on the other runners**, scaled by single-core speed:

| Runner | Core against an M3 | parse | check | dev compile, whole machine |
|---|---:|---:|---:|---:|
| macOS, 12 vCPU, M4 | 1.1–1.2x | 12–13 M | 4.4 M | 390–430 k |
| x86_64, 4 vCPU, EPYC | 0.8–1.0x | 9–11 M | 3.2–4.0 M | 250–330 k |
| arm64, 8 vCPU, Ampere-1a | 0.35x | 4 M | 1.4 M | 120–140 k |

**Each layer catches a slowdown.** Two were injected and reverted:

| Injected | `count` | `wall` |
|---|---|---|
| 100 spins a line in `parser::parse` | parse 2,313 a line, over 1,800: fails | parse 7.14 M lines/s: warns |
| 1 M spins a module in `Checker::run` | check 17,278 a line, over 12,000: fails | check 850 k lines/s: fails |

A failure names the phase, the corpus, the measured figure and the goal:

```text
error: check retires 17,278 instructions a line on pinned:mixed-1M, over its budget of 12,000. At the reference core's 12 G instructions a second that's 694.5k lines/s, under the goal of 1.00M lines/s.
```

How each counter works:

- **macOS** counts a child's own thread around the phase
  (`profile::thread_instructions`), so reading the corpus stays out. The
  kernel's page faults stay in, which moves a count 1–2% with memory pressure,
  so the fewest of three runs counts. The build's count is `time -l`'s.
- **Linux** runs each child under cachegrind in §9's `perf` shell, pinned to
  one core, so the compiler starts no workers. It counts user code only and is
  exact. Reading the corpus is a child of its own, subtracted from parse, and
  so is loading it, subtracted from check.
- **A virtual machine counts nothing** on macOS, which is why the counted job
  runs on Linux. The gate says so rather than reading zero.

The corpora are written to `target/goals` (or `BURI_GOALS_DIR`) once and reused
while their bytes match the pinned digest. CI caches that directory.

**Leads, measured and left:**

- **A build's own `lex+parse` phase was 2.1x the parser's work**: 3,400
  instructions a line on `mixed-1M`, against `parser::parse`'s 1,675. The goal
  covers the parser, as §4's seam does. §6.83 takes both down.

### 6.83 Parsing, and a build's lex+parse, 2026-10-09

One M3 Pro, release build, load 2–4, before at `483013296` and after at this
section's commit. `parser::parse` and `lexer::lex` run over every module of
each pinned 1M shape, fewest instructions and fastest of seven. A load is
`driver::load_all` of the shape as a repository, whole-process instructions
and fastest of three. A build's `lex+parse` is `BURI_PROFILE=1` on a cold
`buri build`.

**Where the work went.** `mixed-1M` is 6.95 tokens a line:

| Before | Instructions a line |
|---|---:|
| `lexer::lex`, 111 a token | 772 |
| the parser over those tokens, 145 a token | 1,008 |
| **`parser::parse`** | **1,780** |
| reading 3,473 files, mostly in the kernel | 606 |
| resolving imports: three `stat`s per import line | 526 |
| a line table for every file read | 157 |
| parsing into memory not yet touched | 90 |
| a second `stat` per file read | 44 |
| the rest of the walk | 113 |
| **a load** | **3,316** |

Neither half had a hot spot. A one-letter word cost 101 instructions to lex, a
`(` 54, a `///` line about 360, an integer 200; `x;` cost 446 to parse, spread
over a dozen calls of a few dozen instructions each. Nothing lexes twice: the
formatter and the doc extractor read the parse, and `flat.rs` builds the tree in
place.

**What changed**, `mixed-1M`:

| Change | Before | After |
|---|---:|---:|
| A load resolves each import path once | load 3,316 | 2,790 |
| A line table waits for a line to be asked for; one `stat` per read | load 2,787 | 2,586 |
| The lexer: cursor and tokens in locals, a class table to dispatch, a word read from its key's load, small integers valued while scanned, comment and string ends found eight bytes at a time | `lex` 772 | 606 |
| The parser: a plain operand, type or binding in one step; the cursor's kind and location kept beside it | `parse` 1,613 | 1,478 |
| A load reads and parses a module's imports side by side, on a third of the cores | load wall 208 ms | 64 ms |
| The lexer's trivia test from a register; a two-instruction keyword hash | `lex` 606 | 592 |

**Every pinned 1M shape**, before → after:

| Shape | `parse`, instructions a line | `parse`, M lines/s | `lex`, instructions a line | load, instructions a line | load, M lines/s | build `lex+parse`, instructions a line |
|---|---:|---:|---:|---:|---:|---:|
| `comment-free` | 2,141 → 1,762 | 9.1 → 10.2 | 886 → 679 | 4,007 → 2,852 | 3.9 → 12.9 | 4,096 → 3,194 |
| `comment-heavy` | 1,268 → 1,057 | 15.7 → 18.1 | 692 → 559 | 2,240 → 1,570 | 7.0 → 25.1 | 2,252 → 1,710 |
| `derive-heavy` | 1,700 → 1,401 | 11.5 → 12.9 | 744 → 566 | 3,211 → 2,287 | 4.8 → 15.7 | 3,275 → 2,531 |
| `enum-heavy` | 1,739 → 1,378 | 12.0 → 13.3 | 713 → 528 | 3,190 → 2,213 | 4.9 → 16.4 | 3,264 → 2,476 |
| `generic-blowup` | 1,911 → 1,607 | 10.5 → 11.7 | 789 → 619 | 3,562 → 2,520 | 4.5 → 14.9 | 3,551 → 2,793 |
| `generic-free` | 1,751 → 1,436 | 11.2 → 12.6 | 761 → 580 | 3,287 → 2,330 | 4.7 → 15.7 | 3,345 → 2,576 |
| `impl-heavy` | 1,634 → 1,317 | 12.4 → 14.0 | 732 → 537 | 3,001 → 2,125 | 5.3 → 17.1 | 3,107 → 2,380 |
| `list-heavy` | 1,837 → 1,497 | 10.9 → 12.3 | 804 → 607 | 3,387 → 2,415 | 4.7 → 15.3 | 3,491 → 2,691 |
| `long-bodies` | 1,839 → 1,426 | 10.7 → 12.7 | 762 → 548 | 3,745 → 2,655 | 4.0 → 14.2 | 3,477 → 2,620 |
| `long-idents` | 1,622 → 1,338 | 12.2 → 13.6 | 737 → 572 | 3,061 → 2,142 | 3.8 → 17.2 | 3,070 → 2,364 |
| `match-heavy` | 1,651 → 1,337 | 12.4 → 13.8 | 723 → 530 | 3,211 → 2,233 | 4.1 → 16.4 | 3,268 → 2,498 |
| `mixed` | 1,779 → 1,462 | 11.1 → 12.4 | 772 → 591 | 3,324 → 2,359 | 4.6 → 15.8 | 3,374 → 2,607 |
| `mixed-deep-graph` | 1,779 → 1,462 | 11.0 → 12.4 | 772 → 591 | 3,462 → 2,368 | 4.5 → 15.6 | 3,386 → 2,621 |
| `mixed-few-files` | 1,673 → 1,339 | 11.8 → 13.6 | 730 → 546 | 1,902 → 1,421 | 9.8 → 36.2 | 2,010 → 1,532 |
| `mixed-libs` | 1,777 → 1,460 | 11.0 → 12.4 | 771 → 590 | 3,446 → 2,366 | 4.5 → 14.2 | 3,379 → 2,611 |
| `mixed-many-files` | 1,952 → 1,687 | 10.2 → 11.1 | 857 → 683 | 9,918 → 6,571 | 1.4 → 4.2 | 9,020 → 6,553 |
| `mixed-wide-graph` | 1,831 → 1,511 | 11.1 → 12.4 | 794 → 608 | 4,852 → 2,667 | 3.3 → 13.0 | 4,364 → 2,825 |
| `string-heavy` | 1,948 → 1,620 | 10.4 → 11.5 | 849 → 672 | 3,504 → 2,537 | 4.6 → 15.0 | 3,579 → 2,784 |
| `struct-heavy` | 1,363 → 1,115 | 14.7 → 16.3 | 649 → 489 | 2,658 → 1,862 | 5.9 → 19.2 | 2,708 → 2,068 |
| `struct-light` | 1,800 → 1,476 | 11.3 → 12.6 | 774 → 593 | 3,347 → 2,374 | 4.7 → 15.8 | 3,409 → 2,630 |

A build's `lex+parse` phase uses less CPU than before on every shape but
`mixed-deep-graph`, where it's level: 0.196 s against 0.218 s on `mixed-1M`,
0.579 s against 0.789 s on `mixed-many-files-1M`.

**The goal.** `parser::parse` holds it on every shape, from 1.02× on
`comment-free` to 1.81× on `comment-heavy`, and 1.24× on `mixed`. Three times
the goal is
about 600 instructions a line at this core's 18 G a second, and the lexer
alone is 592. What's left is spread thin: a `(` still costs 42 instructions to
lex, a one-letter word 68, and an operand goes through a call or two of large
frames. Three times needs a lexer and parser that keep their state in
registers end to end, a different design rather than more fast paths. A load
holds the goal by the clock on every shape but `mixed-many-files`, at 4.2 M
lines a second; per core it's still 2,359 instructions a line on `mixed-1M`,
606 of them reading files.

**A load reads ahead, and the walk is unchanged.** When a module's imports
name two or more files it hasn't loaded, they're read and parsed side by side
under a placeholder file id, and the walk loads each in turn as before.
`Module::refile` moves a parse to the id the source map then gives it, and
`language/corpus.rs` holds that to parsing in place, over every `.buri` file
in the repository. The loads of all 429 test repositories are byte-identical
to before.

**Identity.** Trees, diagnostics, formatted output and `program_text` are
byte-identical on the repository's 8,008 `.buri` files and twelve damaged
copies of each, 1,574 lexer edge cases, all twenty pinned 1M shapes, and
100,000 generated inputs against a build of `483013296`, nesting and chains at
their limits included.

**Guards.** `build/loading.rs` bounds `parser::parse`, and a whole load, in
instructions a line on all twenty shapes at 30,000 lines. It asserts nothing
where the kernel counts nothing. End to end: `language/lexing.rs` (where words,
integers and comments end), `language/parsing.rs` (operands and postfix
chains; nesting and chains one past their limits), `build/loading.rs`
(imports reported where they're written; modules read side by side load as
one at a time). §9's `build/mixed-10k` and `lint/mixed-10k` will read
*improved*; re-bless them.

**Measured and left:**

- Half the cores reading ahead instead of a third: a sixth less wall time,
  for 1.4–2× the CPU.
- `expr`, `ty` and `pattern` inlined into their callers with their fast
  paths: no fewer instructions, and no less time.
- `bump`, `peek`, `eat` and seven other token helpers forced inline: 1,546
  against 1,548.
- A cheaper `Save`. A statement's rollback point costs 43 instructions, 2.5%
  of `parse` on `mixed`; it waits for a change that can shrink `Mark`.

### 6.86 A lexer and parser rewrite for three times the goal, judged, 2026-10-09

Not done. Without `unsafe`, with this tree and with today's recovery, no
design reaches 30 M lines a second on 19 of the 20 pinned shapes, and a
rewrite reaches two thirds of that at best. The bar was three times the goal
with code that isn't significantly more complex.

M3 Pro, release build, `c841da6e6`, fewest instructions of seven. Three times
the goal is about 600 instructions a line at this core's 18.3 G a second.

**The floor is over the budget.** Two passes that do less than any parser
can, both in safe Rust with checked access as the workspace's lints
require: a lexer with no trivia, no cooked text and no diagnostics, and a
pass over its tokens that only writes a tree about the size `parse` writes,
a node per operand, operator and `)`, a kid per `,`, a statement per `;` and
a type per `:`.

| Shape | `parse` today | Lexer floor | Write floor | Floor |
|---|---:|---:|---:|---:|
| `comment-free` | 1,762 | 535 | 268 | 803 |
| `comment-heavy` | 1,059 | 497 | 121 | 618 |
| `derive-heavy` | 1,403 | 475 | 200 | 675 |
| `enum-heavy` | 1,378 | 449 | 221 | 670 |
| `generic-blowup` | 1,607 | 531 | 274 | 805 |
| `generic-free` | 1,436 | 488 | 207 | 695 |
| `impl-heavy` | 1,325 | 451 | 211 | 662 |
| `list-heavy` | 1,501 | 539 | 223 | 762 |
| `long-bodies` | 1,429 | 467 | 207 | 674 |
| `long-idents` | 1,341 | 496 | 189 | 685 |
| `match-heavy` | 1,336 | 453 | 195 | 648 |
| `mixed` | 1,463 | 502 | 214 | 716 |
| `mixed-deep-graph` | 1,466 | 502 | 214 | 716 |
| `mixed-few-files` | 1,339 | 495 | 192 | 687 |
| `mixed-libs` | 1,473 | 503 | 214 | 717 |
| `mixed-many-files` | 1,703 | 513 | 269 | 782 |
| `mixed-wide-graph` | 1,518 | 511 | 221 | 732 |
| `string-heavy` | 1,624 | 546 | 223 | 769 |
| `struct-heavy` | 1,117 | 385 | 158 | 543 |
| `struct-light` | 1,481 | 510 | 220 | 730 |

Instructions a line. Only `struct-heavy` fits. A token record costs 13
instructions to push even into a vector already sized and touched, 16 into a
fresh one, and today's lexer costs 1.12–1.34× its floor.

**A clean parser buys about 40%.** A prototype in the §6.83 style, with
precedence climbing, small frames, cursor in locals and the tree written
directly, covers functions, `let`, expression statements, calls, fields and
named types. It builds a byte-identical tree on those inputs and has no
recovery, no depth or chain budget, and no checks for stray, early or
exchanged tokens.

| Input, per token | Today's parser | Prototype |
|---|---:|---:|
| `let a = x;` | 124 | 56 |
| `let a = x.y;` | 123 | 59 |
| `let a = x * y;` | 124 | 77 |
| `let a = f(x);` | 137 | 79 |
| `x;` | 189 | 101 |
| `fn f(a: Int, b: Int): Int { 1 }` | 106 | 65 |
| `let a1 = str.format(ctx, r.f0, x.y.z(1, 2));` | 121 | 80 |
| `let a1 = a0 * 79 + 3;` | 134 | 96 |

The lexer is 57–76 a token on the same inputs, and isn't counted above.

**The best a rewrite reaches** is the lexer at its floor and the parser at the
prototype's ratio, 0.45–0.72. That's 894–1,025 instructions a line on `mixed`,
1.8–2.0× the goal, and 1,022–1,185 on `comment-free`, 1.5–1.8×. It's an upper
bound, since keeping recovery byte-identical keeps its checks on the happy
path.

**The complexity is the other half.** `lexer.rs` is 1,502 lines of code,
`parser.rs` 3,028, `flat.rs` 1,197 and `tree.rs` 367, tests and comments left
out. About 600 of the parser's lines are recovery helpers alone, and every
recovery path is pinned by diagnostics goldens and the formatter. The designs that would get past the
prototype cost more code that's harder to follow:

- Lexing on demand drops the token buffer, 13 instructions to write a token
  and a few more to read it back. The parser looks ahead by index, scans up to
  256 tokens for a closer or a `>`, recounts a failed construct's
  delimiters over its tokens, and rewinds after a trial parse, so it
  needs a ring buffer with rewind. The formatter still wants the buffer.
- An explicit-stack parser keeps the cursor in registers across a whole
  expression, and turns every production and every recovery path into a
  state machine.

**What would move it**, all outside the constraint: `unsafe` for unchecked
token and arena access, a tree that holds spans in its nodes, or recovery
that's allowed to say something different.

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
- **A compiled program's large blocks used to move with the machine.** macOS's
  `malloc` keeps a freed block past a few KB until the kernel reclaims it, on
  the kernel's schedule. A loop that dropped and remade a 288 KB list then got
  it back for nothing on one run and paid two Mach calls a turn on the next:
  `collection_costs`' `map` over 2,000 records retired 19.6 M instructions or
  30.7 M, and `filterMap` read 342 instructions an element instead of 171 in
  a full suite run. Holding memory short with `memory_pressure -p 15` made it
  happen on most runs. The runtime now keeps a thread's last few large blocks
  itself (`memory.rs`'s large blocks, at most 16 MB a process), so the counts
  hold still under that pressure.

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
  floor. CI's regression gate uses it (§9).
- **kpc and per-thread cycle counters beyond the fixed two** need root.
  `thread_selfcounts` gives instructions and cycles, which is all this needed.
- **Process deltas at phase boundaries.** `proc_pid_rusage` is per process, and
  `buri test` checks one suite while it links another, so a phase boundary
  isn't a moment in time.


---

## 9. The instruction-count gate

CI's `instruction counts (x86_64)` job runs fixed workloads under cachegrind
and fails when one executes more than 2% more instructions than
`cli/benches/instructions/baseline.txt` says:

```text
nix develop .#perf -c cargo bench -p buri --features backend-llvm --bench instructions
```

```text
workload                  baseline             now    change
run/lists               1933542599      2005818604   +3.738%  REGRESSED
run/maps                1986711623      2008860271   +1.115%
run/tree                1527411046      1590324651   +4.119%  REGRESSED
```

Cachegrind counts every instruction it runs, so a rerun reads the same number
to within a few thousand in a billion. Time never decides anything here. The
growth bounds in §6 check how a cost scales, on macOS's kernel counters; this
checks what fixed work costs, on Linux.

| Workload | What cachegrind counts |
|---|---|
| `build/mixed-10k` | a cold `buri build` of `cli/benches/corpora/mixed-10k` |
| `lint/mixed-10k` | a cold `buri lint` of the same |
| `build/programs` | a cold debug `buri build` of `cli/benches/instructions/programs`: stencils and link orchestration |
| `test/one-edit` | `buri test` after adding one test to `cli/benches/instructions/tested`, whose suites already passed |
| `run/<name>` | one program from `cli/benches/instructions/programs`, built with `--release` |

- Every measured process runs under `taskset` on one core, so neither the
  compiler nor the runtime starts workers. Children aren't counted: the linker
  and test binaries cost nothing here.
- Every process gets the same paths and environment under
  `/tmp/buri-instructions`, so every machine hands the compiler the same bytes.
- `~/.buri`'s linker records are filled before anything is measured. A user
  pays for them once.
- CI runs each workload twice (`--runs=2`) and fails if the two disagree by
  more than 0.1%. Measured on an arm64 VM, three runs each: the compiler
  workloads spread by at most 0.0013%, the programs by at most a dozen
  instructions.

### Reading a failure

- **REGRESSED**: find the cause before re-blessing. `cg_annotate
  /tmp/buri-instructions/out/<workload>.cachegrind` ranks functions, and §8's
  protocol compares two builds.
- **improved**: a count fell by more than 2%. It passes. Re-bless so the
  baseline keeps the gain.
- **The toolchain changed**: the baseline's `toolchain` line names rustc, LLVM,
  clang, valgrind and glibc, and counts move with any of them. Bumping
  `flake.lock` or `rust-toolchain.toml` needs a re-bless.
- **moved between runs**: something the workload does isn't repeatable. Find
  it; a wider threshold only hides it.

### Re-blessing

Re-blessing rewrites `baseline.txt`, so a count that rises is a line in review.

- From CI: a failed run uploads `instructions-baseline`, the file as that run
  measured it. Commit it.
- On x86_64 Linux: add `-- --bless` to the command above.

### Running it locally

It's Linux only, because cachegrind is. On macOS, use a Linux VM with nix:

```text
limactl start --name=perf --vm-type=vz --cpus=8 --memory=16 template:ubuntu-24.04
```

Install nix inside it, copy the checkout in, and run the command above. An
arm64 VM counts arm64 instructions, which the committed baseline doesn't have,
so compare two commits: `-- --bless` on the base, then run on yours. Don't
commit the `aarch64` lines; CI checks x86_64 only.

- `-- --runs=3` measures each workload three times and reports the spread.
- `-- run/` runs only the workloads whose names contain `run/`.

### Why x86_64 only

Valgrind's amd64 port shows the program one of a few fixed CPUs, so glibc and
the standard library pick the same code paths on every runner. The arm64 port
works and is repeatable on one machine (the figures above), but glibc picks
its aarch64 `memcpy` by CPU model, and whether the arm runners all read the
same hasn't been checked. Adding them is one matrix line once it has.
