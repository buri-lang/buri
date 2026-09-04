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

They are **goals, not claims**. Semantic analysis and both lowering paths meet
theirs — the native one since 2026-08-29, its first time; lex+parse does not,
and §6 records by how much. `cli/benches/compiler.rs` is what keeps saying so.

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
- **One documented deviation, above 500,000 lines.** The scale tier (§4) takes
  at least **3** repetitions rather than at least 10, and one warmup call rather
  than two. Everything else is unchanged, including the three-quarter-second
  sampling floor — which is never the binding rule at that size, so the cheap
  phases still take nine or ten repetitions and only the expensive ones fall to
  three. The reason is arithmetic: native lowering at a million lines is thirty
  seconds a repetition, so the ten-repetition rule would cost six minutes for
  one row and about forty for the tier. Rows taken under the deviation say so,
  in the table and in `--json`'s `protocol` field, because a deviation nobody
  can see in the output is a deviation nobody can account for. What it costs is
  the dispersion column: a MAD over three samples is a much weaker statement
  than a MAD over ten, and the scale rows should be read for their order of
  magnitude rather than their last digit.
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

So the suite runs three kinds of corpus, and each is answerable for something
the others cannot promise:

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

**Digest-pinned** — `cli/benches/pinned/<name>.txt`, a manifest with no source
beside it. It records what a saved corpus's does — profile, parameters, seed,
generator revision, counts — and the SHA-256 of the bytes that combination
produced; the harness regenerates the corpus on every run and checks the digest
**before it measures anything**. This is what buys byte-stability *at a scale a
git history cannot hold*: the 100,000-line corpus is 3.5 MB and the
million-line one is 35 MB, against a repository whose whole history is 15 MB,
and the manifest for either is four hundred bytes.

There are forty of them, and forty is not forty arbitrary corpora: it is
**twenty parameter points at two scales**, a point being a name and a seed and a
delta from `Params::default()`, and its two corpora sharing that seed so that the
only thing differing between a point's 100k row and its 1M row is the size. The
whole set costs 15,546 bytes of git — `cat cli/benches/pinned/*.txt | wc -c`,
2026-09-01 — which is the argument for the kind stated as a number. §4 lists
the points and what each moves.

The reasoning is that the two properties a saved corpus bundles together are
separable. "These are the bytes" is worth checking in; "here they are" is what
costs the megabytes. A digest gives the first without the second, and the check
it enables is strictly the same one — `corpus::digest` is one function and both
kinds go through it, so pinning a corpus that is *also* checked in produces the
same hash, which is how the generator's byte-identity across a change is
verified in practice.

What it gives up is the reviewable diff, and that is the whole of the cost.
When a saved corpus moves, the diff says *what* moved, in Buri. When a pinned
one moves, the failure has two hashes in it and the counts beside them — which
is why the manifest records `lines`, `bytes` and `modules` as well, so that a
mismatch can at least say whether the shape changed or only its contents.
Recovering the rest means regenerating both revisions by hand. That trade is
right for the scale tier and wrong for a 1,000-line corpus, which is why both
kinds exist rather than one replacing the other.

All three kinds obey the same validity rules, without exception:

- **All are compiled before any is measured.** A saved corpus that has stopped
  being valid Buri is a build failure, exactly as a drifted generator is.
  `--validate` covers the saved half whatever `--set` was asked for, and CI runs
  it. How much of the *pinned* half it covers is `--set`'s business, because
  regenerating and digesting forty corpora, half of them a million lines, is
  three minutes and a plain `--validate` has to stay the check somebody takes
  before a commit. So: none under `--quick`, which is the CI gate and has to stay
  under a second and is 0.4 s; the anchor — `mixed` at both scales — under a
  plain `--validate`, which is what it covered when there were only two
  manifests and is thirteen seconds; the sample under `--validate --set=scale`,
  twenty-seven seconds; all forty under `--validate --set=scale-full`, four
  minutes and twenty. Those four were re-measured on 2026-09-01 and §4 says
  under what conditions. The rule that does not bend is the last row: **the
  whole pinned half is checkable by one documented command**, and a re-pin is
  what happens when it fails.
- **All are in memory before any timer starts.** A saved corpus is loaded, and a
  pinned one regenerated *and* digest-checked, into the same `Program` a
  generator returns; the harness has one measurement path, and no file is read
  inside a timer.
- **All must be reachable from `main`.** `--validate` reports the monomorphized
  function count for each, for the reason the next-but-one rule gives.
- **All are stress-or-realistic, never both.** The family is a property of the
  profile and a saved or pinned corpus inherits it; the goal column is printed
  only for the realistic family, and `Family` is a type in `generate.rs` rather
  than a convention, so the rule is unrepresentable-to-violate rather than
  merely written down. With one derivation on top, for the parameter points the
  scale tier introduced: a corpus whose `params` move anything its profile does
  not **is** a stress shape, whatever family the profile it is a delta from
  belongs to. A point is one dial pushed until it is most of the corpus, which
  is this section's own definition of the stress family, so `mixed` with
  `w_string_fn=8` is quoted against no goal — and that is derived from the
  manifest rather than remembered by whoever pinned it.
- **None is allowed to become the only one.** The headline scale — 100k lines —
  is generated *and* pinned, and the saved anchor is 10k. So §6 records **both**
  the generated and the saved reading of `mixed`, and the two deltas are
  compared: when the compiler changes, both move together; when the *generator*
  changes, only the generated one moves. That pairing is what replaces the
  guarantee the old rule was reaching for, and it is stronger than either corpus
  alone. The pinned 100k row is the third leg of it: it is the same bytes as the
  generated 100k row, checked, so the two agreeing is the pinning scheme
  reporting that it works.

And one rule that applies only to the two kinds with a manifest, because it is
the failure mode a recorded corpus has and a generated one does not:

- **Re-recording is a break in the series, and it is announced.** A saved corpus
  is re-recorded, and a pinned one re-pinned, only when it stops compiling or
  when the generator revision it names is retired; it bumps `revision` in the
  manifest, `--json` carries `corpus_revision`, and §6 says which revision its
  numbers were taken at. A corpus that cannot be regenerated is deleted, not
  repaired. `cli/benches/corpora/README.md` and `cli/benches/pinned/README.md`
  are the operational form of this, with the caps — 512 KiB per corpus, 2 MiB in
  total — that keep the saved half small. The pinned half has no cap because it
  has no size: it is a manifest.
- **A pinned digest that does not match stops the run.** Not a warning and not a
  note. A generated corpus that has drifted out of the language announces itself
  by failing to compile; a generated corpus that has drifted *within* the
  language announces itself here, and nowhere else. The failure prints both
  digests and both sets of counts, and the fix is either to find what moved in
  the generator or to re-pin deliberately.

**Validate before measuring.** Every generated program is compiled through the
real front end — loader, checker, and all — and the suite exits non-zero if it
does not compile. *A benchmark over source that does not compile is a benchmark
of the error paths.* Carbon asserts `!buffer.has_errors()` inside each lexer
benchmark for exactly this; here the check is one level up, over the whole
corpus, before any timer starts.

**A cell that drives the binary starts from a cache no other binary wrote.**
This one is not about the bench target — which compiles in process and keeps no
cache — but about the end-to-end cells §6 quotes, and about any harness that asks
whether an incremental build is byte-identical to a `--force` one. A cache key
carries `arguments::VERSION`, which is `CARGO_PKG_VERSION`: a *version*, not a
hash of the running executable. So rebuilding `buri` at the same version moves
no key, and the first build in a workspace whose `.buri` a previous binary wrote
is a mix of the two compilers' objects — every unit whose IR did not move is
served from the old one. It is a single build: the second agrees with itself,
which is what makes the reading look like noise. Fresh tree, or `--force`, or
`buri clean`, before a cell that spans a compiler rebuild. `buri docs
build/hermeticity`, "The toolchain in the key", is the same warning where a user
of the toolchain will find it.

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

**Many modules, not one file.** At 100,000 lines the mixed corpus is 348
modules with a real import graph — `--validate`'s own count at generator
revision 7, down from the 389 this line used to quote because a laid-out module
reaches its line target with fewer declarations (§6) — each module calling into
one to three others' functions *and* naming one of their types. Three reasons,
all Carbon's: it stops
branch prediction from memorizing one file's shape, it gets closer to the
cache-cold behaviour that matters in practice, and it avoids anchoring on a
single file that may be unrepresentative. A fourth is specific to this
toolchain: cross-module resolution is where semantic analysis would be
superlinear if it were superlinear anywhere, and one big file would never
exercise it.

**Three orders of magnitude in the default run, four on request.** 1k, 10k, 100k
lines by default; the scale tier adds 1M behind `--set=scale` (§4). A single
scale point cannot show a cache cliff, and Carbon's numbers fall 6.70 → 5.02 M
lines/s between 1k and 256k — the fall-off *is* the finding. The default run
stops at 100k for wall time and not for principle, which is why the fourth
order of magnitude is a flag rather than an absence, and §6.4's first finding
is what it found the first time it was taken.

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

- **~~Speed-of-light calibration benchmarks.~~ Adopted since.** Carbon
  benchmarks `strcpy` and a tail-call byte-dispatch loop in the same binary, to
  bound what the hardware can do at all; without them "232 MB/s" is
  uninterpretable. This suite had no equivalent and this list said it should get
  one before anybody worked on the lexer. It has: `cli/benches/calibrate.rs`,
  five loops over the same corpus text the timed rows use — `memcpy`,
  byte-scan, token-write, node-write, alloc-pair — behind `--calibrate`. Its
  interpretation rule was written down before the numbers arrived and the
  binary applies the rule itself, so the reading is not chosen after seeing the
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

`autobenches = false` in `cli/Cargo.toml` is what keeps the first three
*modules* of the `compiler` target rather than bench targets of their own: Cargo
infers a target from every `.rs` file directly under `benches/`, and three extra
binaries with no `main` is what a plain `cargo bench -p buri` would otherwise
try to build.

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
  --rss             peak resident set size per phase, untimed
  --calibrate       the speed-of-light ceilings, per corpus (§3.2)
  --alloc           allocations per line, per phase, untimed
```

`--alloc` needs the toolchain built with its counting global allocator, which is
off by default and not referenced by the library at all: the counter is two
atomic increments on every allocation in the process, so a timed row taken with
it on is not comparable with one taken without it.

### The scale tier

```text
cargo bench -p buri --bench compiler -- --set=scale         # the sample
cargo bench -p buri --bench compiler -- --set=scale-full    # all forty
cargo bench -p buri --bench compiler -- --set=scale --rss   # and peak memory
```

Four orders of magnitude is what says whether a rate is a property of the code
or of the cache, and the fourth one costs minutes. So it is opt-in, and it is
**not** in `core` and not in `full`: a default run has to stay something a
contributor takes before a commit. Six things about it are deliberate.

**The corpora are digest-pinned** (§3.1) — forty manifests in
`cli/benches/pinned/`, regenerated per run and checked against their recorded
SHA-256 before any timer starts. A mismatch stops the run.

**Forty is twenty points at two scales, and the points span the generator's
axes rather than repeating `mixed`.** A single profile measured at 1M answers
"does the rate hold as the program grows"; it does not answer "which parameter
the rate is a function of", and that second question is the one a scale tier is
uniquely placed to ask, because every axis that could be superlinear is only
visibly superlinear at the top. So the twenty are chosen to move one axis each:

| Points | The axis |
|---|---|
| `mixed` | The anchor. Every other point is a delta from it, and shares nothing but the generator. |
| `mixed-many-files`, `mixed-few-files` | Module count against module size: 15,437 modules at 1M against 186. |
| `mixed-libs`, `mixed-deep-graph`, `mixed-wide-graph` | Import-graph shape: clustered, deep, wide. |
| `struct-heavy`/`struct-light`, `enum-heavy`, `impl-heavy`, `match-heavy`, `string-heavy`, `list-heavy`, `long-bodies` | Construct-family weight — one kind turned up until it is most of the corpus. |
| `generic-blowup`/`generic-free` | Generics density: 235k monomorphized functions at 1M against 115k. |
| `derive-heavy` | Derive load, which only the native branch pays for. |
| `comment-heavy`/`comment-free`, `long-idents` | Surface: 46 bytes a line against 29, and bytes per token at a fixed token count. |

Sixteen of the twenty are named profiles from the table below; four —
`string-heavy`, `list-heavy`, `long-bodies`, `generic-free` — are the `mixed`
profile with one weight moved, recorded in the manifest's `params` as such. They
are points and not profiles because a profile earns a row in every `--set=stress`
run, and these had earned a scale row and no more. Each point has its own seed,
and its two scales *share* it, so a point's 1M corpus is its 100k corpus's
modules and then some: the only thing that differs between the two rows is the
size, which is the whole comparison.

**A new scale point is a new manifest and nothing else.** The tier is every
`.txt` in that directory, filtered on the manifest's own fields, so a 10M row is
a `--pin=mixed-10M` away and no code change. It is deliberately absent: at the
rates §6 records, one repetition of a 10M native row is about three minutes,
and the question it would answer — whether anything is superlinear — the 100k/1M
pair already answers, twenty times over.

**The protocol deviation is §2's, printed beside the rows it applies to.** Above
500,000 lines: at least 3 repetitions rather than 10, one warmup call rather
than two.

**Native rows are spent, not spread.** Two rules, both about the same
thirty-to-one cost ratio between a native row and a JavaScript one.

The cross triples go to the anchor only. They earn their seat where they cost
two seconds a repetition and settle whether a gap is codegen or
cross-compilation; across forty corpora they are three quarters of the wall
time, and the question has already been answered on `mixed-100k` over the same
generator. Above 500,000 lines nothing takes them, anchor included.

And a native row at all goes to **seven of the twenty points**, recorded as
`native = false` in the other thirteen manifests. The backend is a function of
two things this suite can move — the codegen unit count and the size of the IR
handed to it — so the seven are chosen to span both: `mixed-many-files` (15,437
units at 1M) and `mixed-few-files` (186) at the ends of the first,
`generic-blowup` (235k monomorphized functions) and `enum-heavy` (55k) at the
ends of the second, `derive-heavy` because `middle::derives` runs only on the
native branch and is invisible in every JS row, `struct-heavy` because layout and
the ABI are native-only questions, and `mixed` because it is the anchor. The
other thirteen move the lexer, the parser or the checker, and `lower+js` is the
lowering row that tracks them. This is a *sampling* decision and it is
reversible: `--only=<point> --set=scale-full --targets=macos-arm64` takes the
native row of any of them by hand.

**The wall time is a property of the flag, not of the directory.** Forty pinned
corpora are a parameter sweep, and a sweep at a million lines is twenty-five
minutes — past the point where anybody runs it before a commit, which would make
it a suite nobody runs. So:

| Command | What it covers | Wall time |
|---|---|---|
| `--set=scale` | the sample: the whole 100k tier, plus `mixed-1M` | ~9 min |
| `--set=scale-full` | all forty | ~25 min |
| `--only=<text>` | either of the above, cut to a point or a scale | seconds to minutes |

The sample is "every pinned corpus the standard protocol applies to, plus the
anchor above it", and the threshold it is stated over is the same 500,000 lines
the repetition deviation already uses, so the tier boundary is one number rather
than two. What the sample buys is the parameter sweep at the scale where a
sweep is affordable and the size comparison on the one point the size comparison
is anchored on; what `scale-full` buys is the other nineteen size comparisons,
and those are a thing somebody does deliberately, on a quiet machine, when a
number is about to be written down.

### Peak memory

`--rss` reports the peak resident set size of each phase, and it is the first
data behind §1's note that peak memory is the obvious fourth column. It is an
**untimed pass**, taken before the timers and never beside them.

The figure comes from a subprocess: `--rss` re-runs this same binary once per
phase under `/usr/bin/time -l` and reads the maximum resident set size back.
There is no dependency-free way to ask in-process — Linux has `/proc/self/status`
and macOS has no `/proc`, `getrusage` is behind `libc`, which is not in this
tree and is not worth buying for a column, and `ps -o rss` requires an
entitlement on current macOS. One phase per process is not a workaround but the
measurement: a peak is monotonic, so the peak of a process that stopped after
`sema` *is* the cost of everything up to and including `sema`, and the
difference between two of them is what a phase added. Sampling the current
figure instead would miss whatever a phase allocates and frees inside itself,
which at these scales is most of the question.

`--validate` is the one to run in a hurry: it is what proves the corpus is still
valid Buri after a language change, and it is fast because it compiles each
program once instead of ten times. A generator that has drifted out of the
language shows up there as a list of diagnostics rather than as a benchmark
quietly measuring the error paths — and so does a *saved* corpus that has
stopped compiling, because `--validate` always covers the checked-in half
whatever `--set` was asked for. It also prints which backends this binary has,
which of the requested targets each can emit for, and how much of the 2 MiB
corpus budget is spent.

How much of the *pinned* half it covers follows `--set`, for the wall-time
reason §3.1 gives: **0.4 s** under `--quick` and no digests, **12.8 s** plain
and the anchor's two, **27.3 s** under `--set=scale`, and **4 min 20 s** under
`--set=scale-full` for all forty. Each is the fastest of three runs taken on
2026-09-01 at `0c66339d`, and the machine was never fully idle while they were.
The three readings each: 0.4/0.8/1.0 s, 12.8/13.3/26.0 s, 27.3/36.7/69.1 s, and
4 min 20 s/4 min 42 s/5 min 17 s, taken at one-minute load averages between 9
and 208 on ten cores. So all four are upper bounds, and against the figures
generator revision 7 recorded on 2026-08-31 — 0.4 s, 12 s, 26 s, 3 min 24 s —
the first three hold and **`--set=scale-full` is at least 27% longer**, which is
the one row here somebody should re-take on a quiet machine before quoting it.
`--set=scale-full` is also the command that answers "is every pinned digest
still good", and it is the one to run after touching `generate.rs` — or after
touching `formatting`, which since `GENERATOR_REVISION` 7 is the same thing:
`laid_out` is the last hand every generated module passes through.

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

There is a step between the two, and the scale tier is where it lives: a
**parameter point** is a `--param` delta with a name, a seed and a pinned
digest, measured at 100k and 1M and nowhere else. Four of the twenty points
above are that — `string-heavy`, `list-heavy`, `long-bodies`, `generic-free` —
and the reason they are not profiles is that a profile costs a row in every
`--set=stress` run and every `--quick` run, forever, while a point costs four
hundred bytes and a line in one table.

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
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` — whichever machine
the suite is run on. A cross triple is *more* reproducible than the host one:
the host ISA is inferred from the running CPU's features and a cross ISA is the
baseline for its triple. The refusal to cross-*link* stays where it belongs, at
`link::can_link` and `actions::native_ready`, which is what
`buri build --output=linux/x86_64` on a mac still answers to; no backend states
the same policy a second time.

**A debug row is only takeable where the debug backend has a stencil library for
the triple** (`design/native/CODEGEN-STENCIL.md` §3.2), which is a narrower
condition than the one these rows were written under: the removed backend was
compiled with `all-arch` and answered for every triple by construction. A triple
with no library is a `skipped` row with the backend's own sentence in it, which
is the mechanism the paragraph below already describes. Of the five triples the
suite can be asked for, that is `macos-x86_64` alone: `linux-x86_64` emits and
is timed like the other two natives (§6.1).

`Profile::Debug` selects the copy-and-patch backend and `Profile::Release`
selects LLVM, so a release row is an LLVM row and it is taken only on a
toolchain built with `backend-llvm`. **There is no `#[cfg]` anywhere in the harness**: it asks
`backend::select` and prints `skipped: <diagnostic>` rather than testing a
feature, so the report always says which rows *this* binary could not take
rather than changing shape depending on how it was compiled. Skipped rows go in
their own `skipped` array in `--json` rather than into `rows`, so a consumer of
the row schema is unaffected.

`Backend::missing_intrinsics` is asked before any timer, for the same reason the
corpus is compiled before any timer: a backend that would have failed must not
be measured failing. The two reasons a native row skipped when these rows were
first taken are both closed, and both are worth recording because the closing is
what made the realistic rows measurable:

- **`list.filter`, `list.fold`, `list.mapCtx`.** The debug backend of the day —
  Cranelift, removed 2026-08-29 — had no body for them and the realistic mix
  calls all three, so every realistic native row skipped. Ten of the closure
  surface were emitted by the time it went, and the copy-and-patch backend that
  replaced it emits the surface the conformance corpus needs;
  `Backend::missing_intrinsics` is the question, asked per backend, and
  `design/native/CODEGEN-STENCIL.md` §9 is what the current one does not do.
- **`Too many return values to fit in registers`** on either x86_64 triple, for
  `main`: `Result<(), Str>` is three scalars and the SysV return convention has
  two, and AArch64's eight had hidden it. The fix is a convention rather than a
  special case — beyond `MAX_RET_LEAVES` a result travels through an
  out-pointer the caller passes and the callee writes, which is the rule the
  runtime's C entries already followed. The threshold is a constant rather than
  a question asked of the ISA, so the memory path is the path every test on
  every host walks.

---

## 5. How large the compiler is

The size census, so that "the compiler got smaller" is a claim somebody can
check. Rust under `cli/src`, counted by a script rather than by a line counter
this repository would have to depend on. A line is a comment when it begins with
`//`.

**Re-taken whole on 2026-09-01, at `0c66339d`**, which is the promise the
previous census left: that one still counted `compiler/backend/cranelift`, five
files and 8,634 lines the tree lost on 2026-08-29, and every total in it was
stale by a backend. The `before` column below is that census; nothing in it was
subtracted by hand.

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
`language_server` is **9.5×** what it was — 1,430 lines to 13,564, four files to
twenty-four — and `cli/runtime` is **3.8×**, 6,954 to 26,720, which is the
reactor, the TLS client, HTTP/1.1 and h2, WebSockets, the carrier stacks and the
scoped arenas that §6.6 and §6.7 measure. Against those, the front end moved
little: `parsing` +8.4%, `semantics` +22%, `middle` +12.5%, `backend/js` +12.4%.
The whole of `cli/src` grew 22% while shedding a backend, so the growth in the
areas that survived is 33,768 lines rather than 25,134 — and the goal-bearing
three of them account for 7,472 of it.

And the Buri-language side, which the compiler has to get through:

| | Files | Code | Comment | Blank | Total | before, total |
|---|---:|---:|---:|---:|---:|---:|
| standard library (`core/*`, `ui/*`) | 44 | 5,526 | 5,646 | 1,202 | 12,374 | 6,552 |
| test corpus (`cli/tests/**/*.buri`) | 5,315 | 63,602 | 15,935 | 10,745 | 90,282 | 25,654 |
| shipped documentation (`cli/src/docs/**/*.md`) | 299 | — | — | — | 19,226 | 12,000 |

The test corpus is the row to read twice: **5,315 files against 1,021**, and
2,000 of the new ones are `cli/tests/formatting/generated`, which is a fixture
directory rather than hand-written Buri. The standard library merely doubled,
and it is now 46% comment by line.

The three phases the goals name are **49,000 lines of Rust** between them —
`parsing` at 7.1k, `semantics` at 14.0k, and `middle` plus `backend/js` at
27.9k — against 35,500 at the previous census. That is the surface any
optimization wave has to work on, and it grew by 38% while every rate on this
page stayed inside its own dispersion or improved (§6.1). The **two** remaining
native backends are another 34.8k on top of it, down from three and 38k, and
they are not what the goals measure — goal 3 is a lowering rate, and a backend
that is twice the code for the same rate has spent it on something else.

**The compiler is also something that has to be built and shipped, and the
2026-08-29 removal moved both.** Measured on the machine and toolchain of §6,
from a clean `cargo build --release -p buri` with default features, at the
commit before the removal and the commit after it:

| | before | after |
|---|---:|---:|
| dependencies (`cargo tree -p buri --edges normal`) | 38 | **0** |
| clean release build, median of three interleaved runs | 142.68 s | **73.94 s** |
| `buri`, as linked | 17.57 MB | 22.63 MB |
| `__TEXT.__text` — the machine code in it | 8.37 MB | **3.03 MB** |

The first two rows are the whole of the case for the removal. **The default
toolchain now resolves nothing at all** — the dependency bar in the workspace
manifest is back to zero admitted crates, which is what its own comment claims —
and a clean release build is 68.7 s faster, a little over half what it was. The
38 that went were Cranelift and its transitive closure: eleven `cranelift-*`
crates, `regalloc2`, `pulley-interpreter`, `object`, `gimli`, `indexmap`,
`hashbrown` twice, `syn`/`quote`/`proc-macro2`, and seventeen more. Building the
bench binary halved with them, 2 m 02 s to 1 m 01 s.

**The last two rows have to be quoted together or they mislead.** Machine code
fell by 5.35 MB, to 0.36× what it was, and dependency-derived constant and
linkedit data by 1.1 MB more — and the shipped binary nonetheless grew by
5.06 MB. Both are true and they have one cause: the three baked stencil
libraries are 11.93 MB of `include_bytes!` data, byte-identical in the two
builds, and before the removal nothing in the `buri` *binary* selected the
copy-and-patch backend, so the linker dead-stripped them. That was verified
rather than assumed — three 48-byte probes from the head, middle and tail of
`stencils-macos-arm64.bin` are found in the newer image and are absent from the
older one. After the removal that data *is* the code generator. Quoting the
total alone reads as a regression the removal did not cause; quoting `__text`
alone claims a saving the disk does not see.

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

**Seventy-eight per cent of the toolchain's growth is one file, and it is not
code this repository wrote.** The runtime archive gained 3.18 MB because the
servers program linked a reactor, a TLS client, HTTP/1.1, h2 and RFC 6455
framing into it — the old archive has no `libburi_rt.a.features` file beside it
at all, because the `net` feature that writes one postdates it. Machine code is
8% of the growth and the stencil libraries are 349 bytes of it. The dependency
bar is still what its comment claims: **the default toolchain resolves nothing**,
at both commits, which is the row a reader should check first when a binary
grows by four megabytes.

---

## 6. Where the toolchain stands

Measured on an M-series MacBook (macOS, aarch64, 10 cores), release build, seed
`0x0b001a575eed0001`, protocol as §2. A gap of 1.0 means the goal is met; below
1.0 means it is beaten.

> **The native figures here were re-taken on 2026-08-29, after Cranelift was
> removed.** Every native row below is the copy-and-patch backend's
> (`design/native/CODEGEN-STENCIL.md`), measured against the same rows taken at
> the commit immediately before the removal, on this machine, over the same
> corpora, both binaries built `--release`. The front-end and JavaScript rows
> moved by less than their dispersion and were not expected to move at all: the
> removal touches the native branch and nothing else. Anything not re-taken says
> so where it stands and keeps its own date — `buri test`'s native default is
> 2026-08-21, generator revision 4 is 2026-08-27.
>
> **One command takes this table again.** The previous revision of this
> paragraph said `cargo bench -p buri --bench compiler` could not complete a
> default run at all: it overflowed the main thread's stack on `wide-match/10k`,
> after every `mixed` scale and every realistic profile had finished and before
> that corpus's first timer, so the rows had to be assembled out of `--only=`
> selections. **That is closed**, and it was checked rather than assumed — both
> binaries were rebuilt and run on 2026-09-01 on this machine, each from its own
> tree, each from the bare command:
>
> | | `--validate` | a default run |
> |---|---|---|
> | `f9fffe1c`, 2026-08-29 | **aborts** — `fatal runtime error: stack overflow`, after `deep-nesting/10k` | **aborts**, the same way, 114 s in |
> | `0c66339d`, 2026-09-01 | **exit 0**, the whole corpus compiles, 12.8 s | **exit 0**, 150 s and 153 s on two runs |
>
> So the table is one invocation again, and the dispersion column is what says
> whether the invocation was worth quoting: every `mixed/100k` row below is the
> better reading of three processes whose every MAD is ≤ 2.8%, inside §2's ±5%.
> Anything not re-taken says so where it stands and keeps its own date.

> **Generator revision 7, 2026-08-31 — a break in the series, announced, and
> the first one that moves a corpus's *shape* rather than only its bytes.**
> Every module now leaves `generate.rs` through `formatting::source`, so a
> generated corpus is what `buri format` writes: four spaces, a sorted import
> run, a `derive` above the declaration it is about, and a body the emitter put
> on one line broken where the printer breaks it. §3.1's rule applies and was
> followed — **all eight** saved corpora were re-recorded and **all forty**
> pinned manifests re-pinned. Every one of the forty is a **six**-line change,
> and the two extra lines are the point: `lines` and `modules` moved this time,
> where revisions 2, 4, 5 and 6 moved only `bytes` and the digest.
>
> **The line count barely moved, and that is the number the goals are stated
> in.** A rate in lines/s divides by `lines`, and `lines` moved by **+0.32%** on
> the anchor (`mixed-100k` 100,755 → 101,074; `mixed-1M` 1,007,259 →
> 1,010,518), by less than 0.5% on thirty-eight of the forty pinned corpora, and
> by at most **+1.54%** on any of them (`mixed-many-files-100k`). So every
> lines/s reading below is comparable with one taken at revision 6 to inside
> half a percent — well inside the dispersion the protocol already reports —
> and the table was not re-taken for this revision.
>
> **The byte count moved where the layout is what the point is about**, and
> `modules` fell on nineteen of the twenty points — 360 → 348 on the anchor,
> 361 → 287 on `struct-heavy` — because a module reaches its line target with
> fewer declarations once its bodies are laid out. `struct-heavy` −12.1% of
> bytes and `long-idents` −10.7% are the two large ones: both are dense in type
> declarations, and a `derive` hoisted above the declaration it is about costs a
> blank line less than a `derive` written below it, so the module that used to
> hold it holds another declaration instead. `impl-heavy` −4.0% and
> `long-bodies` −2.4% are the same effect, smaller; `match-heavy` +5.2% is the
> other direction — a match arm gains four columns of indent rather than two.
> Everything else is inside ±2%, and bytes per line moved by less than a byte on
> sixteen of the twenty points.
>
> **Two saved corpora moved further, and both are stress shapes doing what they
> are for.** `wide-match-1k` is 18,722 → 21,698 bytes at an *identical* 1,000
> lines: nothing but indentation, which is the cleanest possible reading of what
> this revision is. `many-small-fns-1k` is 30,492 → 17,205 and 3 modules → 2,
> because `buri format` writes a one-expression function over three lines: the
> shape's per-function line estimate went 2 → 4, so a 1,000-line budget now buys
> 250 tiny functions where it bought 500. That is a real change in what the
> shape stresses, taken deliberately — four lines per tiny function is the
> density a *formatted* repository of them has, and the old estimate would have
> made `--scale=n` mean 2n lines.
>
> **What it buys.** A line rate is only worth quoting over source somebody would
> check in, and until this revision the corpora were the one Buri in the
> repository nobody had laid out — exempt, by a written row, from the gate that
> holds every other `.buri` file to one layout. The row is gone:
> `cli/benches/corpora` is inside
> `cli/tests/language/corpus.rs::every_source_in_the_repository_is_formatted`
> now, and passes. What it is still outside is `BURI_BLESS`, because the fix for
> a drift in generated output is `--record` and not laying the file out where it
> sits.
>
> **What it costs**, and where. Generating a corpus parses and prints it once
> more before anything else reads it, which is about a fifth on top of every
> validation: `--validate` 10 s → 12 s, `--set=scale` 21 s → 26 s,
> `--set=scale-full` 2 min 47 s → 3 min 24 s. All of it is at work-list
> construction, outside every timer, so no measured rate carries it. The other
> cost is a coupling stated rather than hidden: **a change to `formatting` is
> now a change to the generator**, and takes this same ceremony.

> **Generator revision 6, 2026-08-31 — a break in the series, announced, and
> the one that gives revision 5's bytes back.**
> An import that names a surface names the module again, so
> `core/list/lib.buri` is `core/list`. Every generated module imports four or
> five standard library modules and nothing else across a boundary, so every
> import line is nine bytes shorter; the `//bench/mNNNN.buri` imports name files
> inside `//bench` and did not move at all. §3.1's rule applies and was followed
> — **all eight** saved corpora were re-recorded and **all forty** pinned
> manifests re-pinned.
>
> **Three corpora came back byte-identical**, and they are the exact ones
> revision 5's note is about. `few-large-fns-1k` (23,867) and `wide-match-1k`
> (18,722) carry no import at all; `many-small-fns-1k` (30,492) carries only
> `//bench/...` ones. Revision 5 moved all three anyway, because it renamed the
> module *paths* a digest folds, and this revision does not touch a path inside
> `//bench` — so this time their source bytes are unchanged. The other five
> shrank: `mixed-10k` 347,800 → 346,810, `mixed-many-files-1k` 40,168 → 39,691,
> `mixed-1k` 35,741 → 35,615, `derive-heavy-1k` 35,502 → 35,340,
> `mixed-few-files-1k` 37,202 → 37,157.
>
> **Nothing measurable moved with it.** `lines` and `modules` are identical for
> all forty pinned and all eight saved corpora — checked over the diff, not
> assumed: every one of the forty is a five-line change, and the five lines are
> `generator_revision`, `revision`, `recorded`, `bytes` and `digest`. Every
> reading below is still comparable with one taken at revision 5; a rate quoted
> in lines/s is unmoved because the line count is unmoved.

> **Generator revision 5, 2026-08-30 — a break in the series, announced, and
> the first one that moves a corpus holding no import.**
> Every import named a file, so `core/list` became `core/list/lib.buri` and
> `//bench/m0007` became `//bench/m0007.buri`: every generated import line was
> longer by a suffix. §3.1's rule applies and was followed — **all eight** saved
> corpora were re-recorded and **all forty** pinned manifests re-pinned.
>
> All eight, and that is the part the revision-2 note did not anticipate. Six of
> the corpora have imports and their source bytes grew: `mixed-10k` 346,270 →
> 347,800, `mixed-many-files-1k` 39,356 → 40,168, `mixed-1k` 35,570 → 35,741,
> `derive-heavy-1k` 35,295 → 35,502, `mixed-few-files-1k` 37,152 → 37,202,
> `many-small-fns-1k` 30,482 → 30,492. The other two — `few-large-fns-1k` and
> `wide-match-1k` — have **no import at all**, and not one byte of their source
> moved: 23,867 and 18,722 before and after. Their digests moved anyway, because
> a corpus digest folds each module's *path* as well as its text, and the paths
> are what this change is about: `//bench/main` is `//bench/main.buri` now. So
> there was no corpus to leave where it was, and the revision-2 exemption —
> "byte-stability across a generator change is the whole point of saving one" —
> had nothing to protect: the byte stability is intact and visible in the two
> unchanged `bytes` figures, and only the name of the thing those bytes are
> under has changed.
>
> **Nothing measurable moved with it.** `lines` and `modules` are identical for
> all forty pinned and all eight saved corpora — checked over the diff, not
> assumed — and only `bytes` and the digest differ. Every reading below is still
> comparable with one taken at revision 5; a rate quoted in lines/s is unmoved
> because the line count is unmoved.

> **Generator revision 4, 2026-08-27 — a break in the series, announced.**
> An enum variant stopped carrying `export`, so every generated variant line
> lost the keyword and a space and every recorded digest of a corpus containing
> an enum moved. §3.1's rule applies and was followed: the six saved corpora
> that carry an enum were re-recorded — five at **corpus revision 4** and
> `wide-match-1k` at **revision 2**, which is its first move since it was
> written — thirty-eight of the forty pinned manifests were re-pinned, and the
> ones whose bytes never moved — `many-small-fns-1k`, `few-large-fns-1k`, and
> the `struct-heavy` pins at both scales, which set `w_enum=0` — were **left
> where they were**, for the reason the revision-2 note gives.
>
> **Nothing measurable moved with it.** The change deletes bytes from a
> declaration and nothing else: `lines` and `modules` are identical for all
> forty pinned and all eight saved corpora, and only `bytes` and the digest
> differ. Every reading below is still comparable with one taken at revision 4;
> a rate quoted in lines/s is unmoved because the line count is unmoved.

> **Generator revision 3, 2026-08-27 — a break in the series, announced.**
> `self` stopped writing its type, so every generated method signature lost the
> receiver's name and a colon and every recorded digest of a corpus containing
> a method moved. §3.1's rule applies and was followed: the five saved corpora
> that carry a method were re-recorded at **corpus revision 3**, thirty-six of
> the forty pinned manifests were re-pinned at it, and the ones whose bytes
> never moved — `wide-match-1k`, `many-small-fns-1k`, `few-large-fns-1k`, and
> the `enum-heavy` and `struct-light` pins at both scales — were **left where
> they were**, for the reason the revision-2 note gives.
>
> **Nothing measurable moved with it.** The change deletes bytes from a
> signature and nothing else: `lines` and `modules` are identical for all forty
> pinned and all eight saved corpora, and only `bytes` and the digest differ.
> Every reading below is still comparable with one taken at revision 3; a rate
> quoted in lines/s is unmoved because the line count is unmoved.

> **Generator revision 2, 2026-08-23 — a break in the series, announced.**
> `core/cap` was renamed `core/effect`, so every generated module's import block
> is three bytes longer and every recorded digest moved. §3.1's rule applies and
> was followed: the five saved corpora that carry the import were re-recorded
> at **corpus revision 2**, all forty pinned manifests were re-pinned at it, and
> the three saved corpora whose bytes never moved — `wide-match-1k`,
> `many-small-fns-1k`, `few-large-fns-1k` — were **left at revision 1**, because
> byte-stability across a generator change is the whole point of saving one and
> re-recording an unchanged corpus would break its series for nothing.
>
> **Nothing measurable moved with it.** The change is a textual substitution in
> one import line per module: `lines` and `modules` are identical for all forty
> pinned and all eight saved corpora, and `bytes` grew by exactly `3 × modules`
> in every one of them — checked, not assumed. Every reading below was taken at
> generator revision 1 and corpus revision 1 and is still comparable with one
> taken at revision 2; a rate quoted in lines/s is unmoved because the line
> count is unmoved, and a rate quoted per byte would differ by the ratio above.

### 6.1 Where every goal stands

mixed/100k, the authoritative corpus, on the machine and protocol above.
**Re-taken 2026-09-01 at `0c66339d`**, from the default run described in §6's
note; the `2026-08-29` column is what this table said at `f9fffe1c`, and the
column after it is that same commit's binary re-run today, which is what
separates a machine from a compiler.

| Phase | Goal | 2026-08-29, `f9fffe1c` | `f9fffe1c` re-run today | **2026-09-01, `0c66339d`** | Gap |
|---|---:|---:|---:|---:|---:|
| lex | (10 M shared) | 12.06 M | 12.58 M | **12.88 M** | **MET** |
| lex+parse | 10 M | 6.40 M | 6.04 M | **6.36 M** | 1.57× |
| sema | 1 M | 1.32 M | 1.15 M | **1.35 M** | **MET** |
| lower+js | 100 k | 311 k | 284.1 k | **255.0 k** | **MET** |
| lower+macos-arm64 | 100 k | 133.3 k | 126.4 k | **135.2 k** | **MET** |

Two of the three goals are still met, and the third is still met on **both**
lowering backends rather than on the JavaScript one alone. Lex+parse started at
1.45 M lines/s and is 4.4× that now; native lowering started at nothing
measurable, because the realistic corpora could not be compiled natively at all.

**One row fell, and it is `lower+js`.** 311 k to 255 k is −18.0%, and it is two
things rather than one: this machine on a different morning, and the compiler.
Separating them is what the middle column is for — `f9fffe1c`'s own binary,
rebuilt and re-run today, reads **284.1 k** where this table recorded 311 k on
the day, so roughly half the fall is the machine and the rest is the toolchain.
The toolchain's half was then priced on its own by running the two binaries
**A/B/A/B** in one sitting, `--only=mixed/100k`, four processes, 2026-09-01 —
the protocol §6.6 uses — taking each compiler's better median and discarding
every leg whose MAD exceeded §2's ±5%. Both columns below are from that sitting,
which is why its `0c66339d` figure is 248.9 k where the table above, whose
better reading came from the default run, says 255.0 k:

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
it is met by 2.5×, so this is a row to report and not a gate that failed. What
it is not is a volume effect, and three denominators say so at once. Between
the two commits the anchor's monomorphized function count *fell* 13,162 →
12,735 and its emitted JavaScript grew only 1,317,286 →
1,347,614 bytes, so the same row is **+18.3% per monomorphized function** and
**−10.7% per emitted byte**: the emitter is doing more work per function, not
being handed more program. The `ctx` parameter every module function now
threads, `println`'s `Result`, and the actor and carrier lowerings are what
arrived in `backend/js` over those 413 commits, and the census above prices the
file at 7,214 → 8,108 lines. Which of them owns the 12.4% is a profile away
(§7) and is not answered here.

**`lower+macos-arm64` is the row that moved, and it moved because its emitter
was replaced.** Cranelift read 62.2 k lines/s on this machine on the day of the
comparison — 58.1 k when this row was last written down — and the copy-and-patch
backend reads **133.3 k**, which is 0.47× the time and 2.14× the rate. That is
within noise of `design/native/CODEGEN-STENCIL.md` §1's "about 0.43×
Cranelift's", and it is the first time goal 3 has been met natively here. The
series breaks at the change of emitter and is marked rather than continued
through, the same rule §3.1 applies to a generator revision.

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
clear goal 3**, which is worth saying because the row for one of them used to be
a skip; `macos-x86_64` is absent because it has no stencil library and says so
in its own words (§4). `enum-heavy` is the one row here still under the goal,
and it halved like the rest.

**The tuned corpus is not what produced this.** A copy-and-patch result is
re-taken on freshly seeded 100k repositories before it is written down, because
a pinned seed can sit in a local minimum. Five seeds minted for the comparison
and never used before, three shapes between them:

| Shape | Seed | ratio | after, lines/s |
|---|---|---:|---:|
| mixed | `0x29a8206b17c0f001` | 0.53× | 116.0 k |
| mixed | `0x29a8206b17c0f002` | 0.49× | 123.9 k |
| mixed | `0x29a8206b17c0f003` | 0.46× | 133.8 k |
| struct-heavy | `0x29a8206b17c0f004` | 0.50× | 180.9 k |
| derive-heavy | `0x29a8206b17c0f005` | 0.45× | 125.2 k |

The median of the five is **0.49×** against the tuned corpus's 0.467×, a
divergence of 5%, and every one of them clears goal 3 at 116.0–180.9 k lines/s.
`derive-heavy` — the profile `middle::derives` alone pays for, and the one
§6.4's `Show` finding is about — is the *best* of the five rather than the
worst, which is the opposite of what overfitting to `mixed` would look like.

### 6.2 The dev and release configuration, both halves measured

- **Dev: the copy-and-patch backend, whole-binary link, per-unit emit.** It has
  no optimization dial to set — there is no instruction selection, no register
  allocator and no mid-end to skip (`design/native/CODEGEN-STENCIL.md` §1), so
  the two rows below that argue about one are history rather than a setting.
  **Three readings in this bullet are Cranelift's**, from before the flip, and
  none has been re-taken because the emitter under all three has changed: LLVM
  at `-O0` was 2.1–4.9× slower to lower on the shapes that decide it, and
  slowest exactly where the dev backend already missed; `opt_level = "speed"`
  cost 16–95% of native lowering and *lost* 2.6% of runtime (§6.4); and the path
  ran the four kernels at 0.91× of bun. What *was* re-taken across the change is
  the pair of comparisons the change was made for: emission at **0.47×**
  Cranelift's time (§6.1) and the run side at **1.26×** Cranelift's, over the
  four comparable kernels of the six-program series below. Erasing generics in
  the dev profile
  (2–10× measured runtime cost, and a second value model through both backends)
  and moving instantiation placement (worth exactly one unit of blast radius,
  and weak symbols would cost the direct branches) were both refuted by
  measurement, and neither refutation depended on which emitter was under it.
- **Release: LLVM at `-O2`.** **10.55×** a cold dev build and **1.07× a no-op
  one**, for 1.84× the runtime over the four kernels below and 0.35× the
  artifact size — that last from before the flip and not re-taken. It lowers at
  5.5 k lines/s on the headline corpus, inside the 3.9–6.8 k band this row has
  always quoted, which is 18× under goal 3 and is the price of LLVM's optimizer
  rather than of this repository's lowering; the goal is met by the path a
  developer iterates on. The distance between the two profiles widened with the
  removal without the release column moving at all: dev emission is 22.3× release
  emission now, against 11.2× before.

**The end-to-end numbers behind those two ratios.** No 100,000-line repository
ships in the tree and the generator's `--record` refuses one, so it was
assembled out of the checked-in `mixed-10k` corpus: ten copies, one package
each, module paths rewritten — **101,190 non-blank lines, 10 packages, 370
modules**, every package a `MACOS`/`ARM64` binary. Cold is `buri clean` plus
`rm -rf .buri out`; no-op is the same command again with nothing changed.

| Configuration | cold, median | no-op, median |
|---|---:|---:|
| dev, Cranelift (before) | 2.964 s | 0.388 s |
| dev, copy-and-patch (after) | **2.107 s** | 0.398 s |
| `--release`, LLVM `-O2` | 22.228 s | 0.426 s |

The cold build improves by 29% where emission improved by 53%, and the gap
between those two is the finding rather than a discrepancy: at ten packages on
ten cores the build is parallel, and the front end, the action cache and the
link did not move — only the emission share did. So the release-over-dev cold
ratio **rises to 10.55×**, from the 7.9× this row used to record and the 7.50×
the same measurement gives at the old commit today; the no-op ratio is 1.07×
against the 1.15× recorded before. A single 10k package, for scale: 0.272 s
cold and 0.087 s no-op.

**The run side, on six kernels written for the comparison.** The four programs
behind the 1.38× that `design/native/CODEGEN-STENCIL.md` §13 records **have no
harness in this repository** — nothing in the tree reproduces them — so the six
below are a fresh series and their geomean is not that number re-taken. What
they are is the same six sources through three code generators, on one machine
on one afternoon: whole-process wall clock, median of five, macOS arm64. The
LLVM column is `-O2`, because `--release` is the only optimized native path here
and there is no `-O0` one to hold it against; it is a harder bar than the
literature's, not the same one.

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
tightened. Three of the four are within 1.08–1.15× of Cranelift, and **the whole of the remaining gap is
the `core/list` closure pipeline** — the surface `rtcall.rs` deliberately does
not inline, which is a stated exclusion rather than a surprise. Cranelift itself
is 1.46× LLVM `-O2` on the same four, against the copy-and-patch backend's
1.84×, so most of the distance to release is the optimizer rather than the
emitter. The two concat rows are the in-place append port measured: both are at
or better than Cranelift's, which was the point of doing it, and the row whose
left operand stays unique comes out *ahead*. Per append the cost went from
0.659 µs before the port to 0.0098 µs after — 67× less, and growing with bytes
rather than quadratically with appends.

**`buri test` defaults to the native dev backend**, since 2026-08-21. A suite
that names no platform is compiled with the dev backend and run as a binary, and
since 2026-09-03 there is no fallback: a toolchain that cannot build for its own
host, or a program the backend has no body for, is a refusal naming what is
missing rather than a JavaScript run with a note (`commands/test.rs`;
`design/native/ARCHITECTURE.md` §4). The set of hosts that refusal covers got
one member wider on 2026-08-29: the dev backend answers for the triples it has a
stencil library for and no others
(`design/native/CODEGEN-STENCIL.md` §3.2). The
number that paid for the change is the incremental one: a one-line edit at 104k
lines is 502 ms to verdict native against bun's 622 on the fast suite and 1,484
against 1,742 on the compute suite, the first measurement here where the native
compile column is itself the faster one.

### 6.3 What remains open, in the order it matters

- **The `core/list` closure pipeline**, which is now the whole of the dev
  backend's remaining run-side gap: 1.80× Cranelift and 5.51× LLVM `-O2` on the
  fused-pipeline kernel while the other three sit at 1.08–1.15× (§6.2). It is
  `rtcall.rs`'s stated exclusion rather than a defect, and it is the one shape
  where the exclusion costs a reader something visible.
- **`lower+js`'s 12.4%.** The one row on this page that fell over the
  concurrency-and-servers program, measured A/B/A/B against `f9fffe1c` on
  2026-09-01 (§6.1). It is not a volume effect — the same corpus is 3.2% *fewer*
  monomorphized functions and 2.3% more emitted JavaScript, so the cost is per
  function rather than per program — and goal 3 is still met by 2.5×, which is
  why it is here and not above the fold. What it needs first is a profile (§7)
  over the JavaScript lowering call, to say whether it is the threaded `ctx`
  parameter, `println`'s `Result`, or the actor and carrier lowerings; nothing
  in this slice tried to answer that.
- **The producer half of fusion.** `range` is still materialized.
- **Derived `Show`**, which needs the design decision in §6.4 rather than more
  tuning.
- **~~`buri_rt_list_get`.~~ Closed 2026-08-30, on both backends.** It was the
  whole of the matmul kernel's remaining gap — 1.56× when that was measured,
  1.38× against LLVM `-O2` in §6.2's series — and it was an out-of-line call
  that bounds-checks and `memmove`s one element into an `Option` payload, twice
  per inner iteration there. Both backends now open-code it out of the same
  bounds test and load that `list.map`'s loop already uses, and both beat the
  2× this row predicted. The dev backend went first (`4aa877f`, `3ba487e`):
  `matmul` at **0.468×** its old time and `queens` at **0.628×**, with the four
  held-out kernels — written after the fix was committed — at a geomean of
  **0.519×**, a *larger* win than the tuned pair's 0.542×. The release backend
  followed (`3b262681`): **0.255×** over the six kernels that index a list, the
  held-out four at **0.250×**, again ahead of the tuned pair's 0.266×. Neither
  half was tuned against a kernel, and the held-out column saying so twice is
  the reason the numbers are here rather than in a footnote. Landing the dev
  half alone put the dev backend *ahead* of `--release` on exactly those six,
  dev÷release falling to 0.65×, and the release half restores the ordering with
  room to spare, at **2.59×**. A counted element still takes the call on both
  backends: the runtime entry retains through the glue it is handed and the
  open-coded sequence does not, so that half is a reference-counting question
  rather than a codegen one. §6.2's three-generator table predates all three
  commits and is not re-taken here.
- **~~Realistic native lowering's last 1.72×.~~ Closed 2026-08-29.** The
  measurement said 88% of the row was inside Cranelift's own `define_function`
  and 42% regalloc2 alone, so the lowering this repository owned was not the
  cost, and the next step was a value-model change or a different codegen
  strategy rather than a faster loop. **The second one was taken**: the emitter
  under this row is a copy-and-patch one with no register allocator in it at
  all, the row reads 133.3 k lines/s, and goal 3 is met natively for the first
  time (§6.1). It is left here rather than deleted because the reason it closed
  is the finding — the gap was in a dependency's design, and no amount of
  tuning on this side of the seam was going to reach it.
- **Lex+parse's last 1.57×.** The plateau without a design change is
  ~5.5–6 M lines/s; reaching 10 M additionally needs the C3 rewrite. 11.2% of
  the phase is provably unavoidable while a standard-library pin stands. Both
  are product decisions rather than optimizations.

### 6.4 Three findings that transfer

The rounds that produced the numbers above are not kept as a chronology: a log
whose every row is superseded by a later row in the same document is a worse
version of the last row. An earlier revision numbered those rounds §6.1 through
§6.9, so a citation to one of them lands here. Three of the findings are worth
more than the numbers
they produced, because each is a shape rather than a measurement, and they are
the three below.

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
derived `Show` costs 4.7× the entire native lowering row while `Eq`, `Ord`,
`Hash`, `ToJson` and `FromJson` together cost nothing measurable. Three
measurements say what it is not. Generating the expansion costs 29 ms and
lowering it to IR 32 ms more, while *emitting* it costs 1,221 ms more — forty
times the expansion that caused it. Regrouping 512 total variants from
128 enums × 4 to 8 × 64 costs the same to within 3%, so it is not superlinear in
function size and a per-variant helper split will not work. And payload-free
derived `Show` is free (231.0 ms against 234.9 with no derive at all) while one
field costs 33 ms, two 72 and four 163 — about **0.07 ms per hole**. 65% of the
row is `regalloc2`, `Env::init` alone being 18% of the whole process. So the
lever is CLIF volume and live ranges per hole, and the two routes left are a
variadic join that takes its parts through memory or a descriptor-driven
renderer emitted once, which is what the JavaScript backend already does. That
is a decision about `middle::derives`'s premise rather than an optimization, and
it is recorded here because the measurement rules out the two cheaper answers.

**`opt_level = "speed"` on the dev backend was refuted on both halves of the
trade.** It cost 16–95% of native lowering. What it returned, over the four
kernels: primes −3.6%, n-queens −4.3%, matmul ±0%, and the fused pipeline
**+34%** — the one shape the fusion pass had just made fast, regressed by a
third because the egraph mid-end rewrote the fused loop into something its
register allocator liked less. The total was **+2.6%**: the suite slower, not
merely not-faster. The dial belonged to a backend that is gone (2026-08-29) and
the finding outlives it, because it is the shape rather than the number — **an
optimizer in the debug quadrant has to pay for itself on the run side, and this
one did not** — and the backend that replaced it has no such dial to be tempted
by.

### 6.5 Measured dead ends, recorded so they stay dead

- **Branchless reference counting**: +15%.
- **`opt_level = "speed"`** on the dev backend: +20% lowering, and §6.4's
  runtime regression. This and the row below were dials on the Cranelift backend,
  removed 2026-08-29; they are kept because a dead end that is deleted gets
  rediscovered.
- **`regalloc_algorithm = "single_pass"`**: silently inert since Cranelift
  0.123, which withdrew the value — the line was set and had no effect. It went
  with the backend and with the document that recorded it
  (`design/native/CODEGEN-STENCIL.md` §13).
- **Erasing generics in the dev profile**, and **moving instantiation
  placement**: both refuted in §6.2.

### 6.6 What the multi-threaded fork costs, 2026-08-30

Reference counting became **two counts behind one branch** on bit 63 of the
block's `cap` (MEMORY.md §5.1, "The shared fork"). At the time of this
measurement nothing set the bit, so no program took the atomic arm and what
follows is the price of the *branch* alone. **§6.7 is what changed** — a program
that can reach a task boundary now marks every block it allocates — and it
leaves every number below standing, because a program with no `core/tasks` in
it still takes the unshared arm and every corpus measured here is one.

**The stated budget was 3% on every row of `--set=native`, and it is 3% on four
of them.** The fifth, `lower+macos-arm64-release`, carries a budget of its own —
**amended 2026-08-30** to the range measured below, +16.6% … +26.3%. The
amendment is argued at the end of this section rather than here, because a
budget widened to fit a number is only defensible once the number, and what was
bought with it, are both on the page.

Protocol: `--only=mixed --set=native --targets=macos-arm64 --json`, the same
machine and §2's rules, run **A/B/A/B** — the baseline toolchain, this one, the
baseline again, this one again — so that drift shows up as a disagreement
between a compiler's two readings rather than as the difference between the two
compilers. Each cell is the better of that compiler's two run medians, which is
§2's "fastest sample" argument applied across processes. Six corpora at 10k.

| Phase | range over the six | median | budget |
|---|---:|---:|---|
| `lex` | −3.8% … +0.1% | −0.4% | **met** |
| `lex+parse` | −2.8% … +1.3% | −0.3% | **met** |
| `sema` | −4.9% … +8.2% | +2.2% | **met** |
| `lower+macos-arm64` (dev) | −5.1% … +7.6% | −0.4% | **met** |
| `lower+macos-arm64-release` (LLVM) | **+16.6% … +26.3%** | **+21.3%** | **met**, against an amended budget |

The first four rows have no direction — the change touches native lowering and
nothing else, and the front-end rows moving by ±4% in both directions is what
this machine's drift looks like over a twenty-five-minute run. They are the
control, and they are why the fifth row is believed.

**Goal 3 is unmoved.** §6.1's goal-bearing lowering row is
`lower+macos-arm64`, the development backend, and it is one of the four. The
row that moved is the *release* build's lowering, which §6.2 measures and which
no goal on this page is stated against.

**Why the release row and not the dev one.** The stencil backend copies a
stencil per IR operation, and the fork made three stencils longer rather than
making more of them: the emitter's work is unchanged and the row says so. The
LLVM backend hands `opt` its IR, and the fork adds two basic blocks and about
six instructions to *every reference operation in the program* — which roughly
doubles the IR of the single most common operation the emitter produces. The
`default<O2>` pipeline's cost is superlinear in block count, and 21% is what
that comes to.

**Half of it was bought back by making the shared arm a call.** The atomic
sequence can be open-coded in the IR beside the fork, or the fork can call
`buri_rt_incref`/`buri_rt_decref`, which fork again on the same bit and take
the same arm. Both were measured against the same baseline under the same
protocol:

| Shared arm | `lower+macos-arm64-release` | median |
|---|---:|---:|
| open-coded saturating `atomicrmw` | +28.8% … +85.7% | +46.3% |
| **cold call into the runtime** (landed) | +16.6% … +26.3% | **+21.3%** |

The machine code on the arm every program actually takes is identical either
way, so this is a pure win and the call is what shipped. It also leaves one
atomic sequence per backend rather than two, which is MEMORY.md §5.1's
"open-code the fast path, call the cold one" applied to a path that is cold by
construction.

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

The load is of the word next to the count, in the same sixteen-byte header, so
it is on a cache line the operation was going to touch; the branch is perfectly
predicted because its answer never changes; and both emitters mark the unshared
arm hot, so it is the fallthrough and the shared arm is laid out after the tail.

**And what the emitted program does with it: it gets faster.** Two Buri programs
were built by both toolchains and run five times each, alternating — `allocs`,
whose loop is three inlined `decref`s and an allocation per iteration, and
`rcloop`, whose counts `middle::rc` elides entirely, as the control. Best of
five, seconds:

| program | backend | before | after | Δ |
|---|---|---:|---:|---:|
| `allocs` (50 M iterations) | dev / stencil | 1.795 | 1.081 | **−39.8%** |
| `allocs` (50 M iterations) | LLVM release | 1.711 | 0.605 | **−64.6%** |
| `rcloop` (control, no counts) | dev / stencil | 0.423 | 0.431 | +1.9% |

That is a **combined** figure and should be read as one: the fork's two
instructions against the per-thread caches of MEMORY.md §5.4, which turn an
allocation-and-free pair into a list pop and a list push. The caches pay for the
fork many times over on any program that allocates, and the control — which has
neither a count nor an allocation — does not move. What the table cannot
separate is the fork's own runtime cost, because this language has no
allocation-free reference traffic to isolate it in: `middle::rc` elides exactly
that.

**Two of these runs were taken and discarded first.** The machine's load average
reached 136 while other work shared it, and the same binary timed 2.5 s and
20.7 s inside one alternating sequence. §2's ±5% dispersion rule says a run like
that is one to discard rather than to quote, and it was; the table above is from
a later run whose five readings per cell agree to ±8%. The two-instruction count
is the claim that does not depend on either, because it is a claim about the
object rather than about the machine.

#### 6.6.1 The amendment, and what it is answerable to

The 3% rule stands, unchanged, on the other four rows and on every row a later
change to `--set=native` is measured against. What is amended is one row's
budget: `lower+macos-arm64-release` is held to **+16.6% … +26.3%**, median
+21.3% — the range this run measured — **accepted by Nick, 2026-08-30**. Three
things are what it was accepted on, and all three are measured above rather than
argued:

- **What is spent is compile time, and only in one backend.** The release row is
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

What the amendment is **not** is a widening of the rule. A row with a budget of
its own is a row whose next regression is measured against *this range* rather
than against 3%, so a second change of this size here is a second decision and
not a rounding error. The alternative that lost was to hold 3% and leave the row
permanently red: a budget nothing can meet is a budget nobody reads, and it
would have gone on hiding a compile-time cost the runtime table in this section
pays back.

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
`alloc.scoped` around each batch — so it is not a before-and-after at all but
what a scope costs the program that opens one: 500,000 scopes, **183 ns each**,
which is `create`, `enter`, `leave`, `release` and two uncontended mutexes.

#### 6.7.1 Three shapes were measured and two were thrown away

Every number below is the first row of the table above, on the same machine in
the same sitting. They are recorded because each rejection is a fact about this
platform rather than a preference.

| where the "which scope am I in" question lives | `allocs` |
|---|---:|
| a `thread_local!` of its own | **+12.0%** |
| folded into G2's per-thread block cache | +4.1% |
| the same, plus a process-wide "any scope ever" latch | **+3.7%** |

**A second thread-local costs 2.4 ns on an allocate-and-free pair**, which is
about a third of what the pair costs — on macOS a `thread_local!` access is a
call to `tlv_get_addr` and not a register-relative load, and the allocation path
was already making one for the block cache. Folding the arena into
`Cache` makes it one access and a branch on a word already in a register, and it
is the whole of the difference between the first two rows.

The third row is **within the noise of the second, and is kept anyway.**
`scopes_exist()` is a relaxed load of a word written at most once in a process's
life — G3's marking-latch shape — and what it buys is not the 0.3% but the
claim: a program that never calls `scoped` takes the two lines it took before
this slice, and that is readable off `buri_rt_alloc` rather than reasoned about
from a profile.

#### 6.7.2 The pool is why the second row is 8.7% and not 116%

The first working version of the scope **mapped and unmapped a 64 KiB block per
scope**, and the second row measured **2.2554 s against 1.0444 s** — 2.2× the
same program without scopes, or **2.4 µs a scope**, which is two system calls
and nothing else. A feature whose cost is that shape is not a feature.

`ARENA_POOL` is eight standard blocks — 512 KiB, stated and bounded — that a
released scope hands to the next one instead of to the kernel. It is the third
time this runtime makes that trade (G2's per-thread block caches, B7's
carrier-stack pool) and the reason is the same each time: **the common path
should make no system call.** With it, a scope's 2.4 µs becomes 183 ns.

One correctness consequence is worth stating beside the number, because it is
the kind that would not have shown up in a benchmark: a mapping fresh from the
kernel is zero-filled and an arena's window only moves forward, so before the
pool existed `buri_rt_alloc_zeroed` inside a scope could be a bump and nothing
else. A **pooled** block holds the last scope's bytes, so it zeroes.
`a_zeroed_block_in_a_scope_is_zero_even_out_of_the_pool` is that, as a test.

### 6.8 Where the program ends, 2026-09-01

The concurrency-and-servers program — carriers, stack switching, scoped arenas,
HTTP/1.1 and h2 with TLS and WebSockets, actors, the test doubles, and four
flag-days — is complete at `0c66339d`, and the rest of this page is written from
inside it. This section is the end state in one place: what the suite costs to
run, what the toolchain costs to ship, and the two columns §1 promised and §6
never had. Every figure below is from a run on this machine at that commit or
from a script in the tree that carries its own measurement.

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
machine was never idle** — it carried other work throughout, at one-minute load
averages between 9 and 208 on ten cores — so each is the fastest of the runs
taken and every one is an upper bound rather than a quiet-machine figure. The
suite's own spread says how much that matters: four runs of the same 1,157
tests read **2 min 3 s**, 2 min 30 s, 2 min 42 s and **8 min 38 s** of test
execution, at one-minute load averages of 6, 14, 41 and 45.

**One test is over half the suite's wall time, and it is the one that needed a
retry.** `buri::recovery a_syntax_error_does_not_become_a_type_error` read
68.8 s of the fastest run's 123.0 s, 88.2 s and 150.0 s on the next two, and on
the busiest run it ran past `nextest`'s 300-second slow timeout, was terminated,
and passed on the retry. It is the long pole and the only test here that needed
a second attempt in any of the four. A retry is not a fix and this page is not
where that gets fixed, but the suite's wall time is a number this section
quotes, so the test that owns it is named.

**It is fixed, and the two rows above are the last ones taken before it was.**
The test was one serial loop of five thousand seven hundred `analyze_snippet`
calls, and `analyze_snippet` builds a `SourceMap` and a parse cache from
nothing — so each two-hundred-byte mutated snippet re-parsed the whole standard
library. Neither half of that was the corpus's fault: the cases are independent
pure functions of their own text, and both structures are built to be reused.
`cli/tests/recovery.rs` now computes every per-file baseline up front, fans the
cases out over `buri::parallel::map_with`, and gives each worker one `SourceMap`
and one parse cache to keep — so the standard library is parsed once per core
rather than once per case, and the verdicts come back in index order to be
folded into the report on one thread. **Nothing about the population, the
ceilings or the table changed**: the tables print byte for byte what they
printed before. On the machine of §6 the whole `recovery` suite went from
**77.9 s to 9.0 s**, and this test from 65.1 s to 8.9 s. CI's four-core runner
read 162.9 s for the suite and is not re-measured here; what it now has to do is
15% less work, divided four ways rather than run down one.

The row worth pausing on is the digests. Forty pinned manifests were re-pinned
at generator revision 7 and **all forty regenerate to their recorded SHA-256 at
`0c66339d`** — `--validate --set=scale-full`, exit 0 — which is the whole
program's worth of language change landing without moving a byte the generator
emits. That is the check §3.1 exists to make, taken at the end rather than
assumed.

**What the toolchain ships, per platform.** The runtime archive is
`include_bytes!`d into every `buri` binary, so its size is the toolchain's size,
and `.github/scripts/assert-runtime-archive.sh` is the ratchet that holds it.
Its numbers, and the one this tree reproduced on 2026-09-01:

| triple | `net` off | `net` | `net-h3` | budget | headroom |
|---|---:|---:|---:|---:|---:|
| `aarch64-apple-darwin` | 6,329,104 | **9,097,192** | 9,097,432 | 9,437,184 | 3.6% |
| `aarch64-unknown-linux-musl` | — | **13,938,046** | — | 14,680,064 | 5.05% |

The Darwin figure is not quoted from the script: a `cargo build --release -p
buri` in this worktree produced an archive of exactly 9,097,192 bytes, and the
script passed over it. The Linux figure is the script's own, measured in the
container `scripts/test-linux.sh` runs, and it is now the **musl** triple's:
the Linux link is static-musl, and the archive it builds is 13,938,046 bytes
against 13,799,068 for the `gnu` triple it replaced — 139 KB and 1.0% larger,
which is musl's standard library rather than anything this repository did. It
is 1.53× Darwin's for the same code — ELF's price per byte, near enough the
ratio F4 and F7 each measured on `gnu`. Both budgets are ratchets and **neither
is hit**; Darwin's 3.6% is still the thinner of the two, the musl switch cost
Linux the difference between 6.0% and 5.05% and needed no raise, and the script
says in capitals that the next slice to add anything at all is the one that
re-measures it.

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
two rows are *flat* from 10k to 100k — 503.8 against 503.8, and 1,179.0 against
1,174.9 — so the front end's allocation count is linear in the program with no
cliff between the two scales, which is the property a per-token budget is
worth stating over. And `sema`'s row is the prelude floor made visible from the
other side: 29,616 per 1,000 lines at 1k against 14,163 at 100k, converging down
exactly as §3.1's floor argument predicts a fixed cost must.

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
backend's. There is still no
*goal* here — §1 says so and this section does not change it — but the column is
no longer empty, and a future budget has a number to be stated against.

**What the runtime costs is measured beside it rather than here.** §6.6 prices
the shared-reference-counting fork the multi-threaded program needed — two
instructions per reference operation, +21.3% on `lower+macos-arm64-release`
against an amended budget, and a 39.8–64.6% *fall* in an allocating program's
run time — and §6.7 prices a scope at 183 ns, +3.7% on a program that opens none
and +8.7% on one that opens 500,000. Both were taken against the same `allocs`
program, and neither is re-taken here: nothing in this slice touched the
runtime.

### 6.9 Where a real repository's `buri test` goes, 2026-09-03

A maintainer's own monorepo — 18 packages, 177 `.buri` files, 173 codegen
units, 488 test cases — ran `buri test //...` cold in **8.2 s**. The question
asked of it was which stencils the run side was missing. **The run side is
0.15 s of it.** The batched test binary the suite builds executes every block
in 154 ms on this machine; the other eight seconds are the compile, and this
section is what was in them.

The phase column is a timer compiled into a throwaway toolchain build, with
`buri clean` before every run and the minimum of several runs taken per phase —
§2's "fastest sample" rule applied per phase rather than per corpus, because the
machine had other work on it. The wall row is the two *shipping* toolchains,
alternating, and is the number to read as the result.

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

`design/native/CODEGEN-STENCIL.md` §4.2 is what each line changed, and the four
findings are worth separating because only one of them is about threads:

1. **The program was lowered twice.** `objects_named` lowers for the unit keys
   and `emit_units` lowered again for the bytes; each lowering carries a
   whole-program `middle::rc::analyze`. 1.01 s, deleted by handing the first
   lowering over.
2. **`Cycles` was rebuilt per unit** — a walk of every constructor plus a
   Tarjan pass, 173 times. §6.4's first finding is exactly this shape and
   `Layouts::with_cycles` exists for exactly this reason; the LLVM backend used
   it and the stencil backend did not.
3. **A `Layout` was copied per instruction and a `Ty` cloned per reference
   operation.** `Layouts::shared` says in its own doc comment that a caller in
   a loop over instructions must use it. `walk_rc` is that loop and used the
   copying form; so did `MakeStruct`, `GetField`, `GetPayload`, `MakeEnum` and
   `GetTag`. On the emission's critical-path unit this was worth 30%.
4. **Three `format!`s and three hash lookups per emitted machine
   instruction**, for the folded twins `Jit::emit` asks about by name.

**On the bench, `lower+macos-arm64` halves.** §6.6's protocol: `--only=mixed
--set=native --targets=macos-arm64`, run A/B/A/B so that drift shows up as a
disagreement between one compiler's two readings rather than as the difference
between the two compilers, each cell the better of that compiler's two run
medians.

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

**§6.1's goal-3 rate is deliberately not restated from these numbers.** The
table above is a *ratio* between two toolchains measured against each other in
one window, which is what A/B/A/B is for; a goal row is an absolute rate, it is
stated over a wider corpus set than `--only=mixed`, and this machine was not
idle for the whole afternoon. The row moves when somebody takes it the way §2
says to.

**What the ceiling is now.** The per-unit emission is parallel, so it is bounded
by the **largest single unit** — on this repository `core/ordmap`, 11,267
monomorphized functions, which is 1.10 s of the 1.10 s. Splitting a unit is a
build-system question (a unit is a cache key and an object file,
`design/native/ARCHITECTURE.md` §5), so the next win on that line is either
making a function cheaper to emit again or making a unit smaller. **§6.10 is
that ceiling taken down**, by a third route neither of those names: the unit is
not split, its *emission* is. Above it,
`actions::prepare` is now the largest single-threaded phase at 1.40 s, of which
0.79 s is `middle::rc::analyze` — and it is *still* the whole-program analysis
the emitter no longer duplicates, so halving it would be worth as much again.

**Two things this did not touch, and both are worth naming.** The suite's
`link` is 0.40 s for a **102 MB** batched debug binary, written to the artifact
cache and then to disk; and `commands/test.rs`'s `run_blocks` re-`exec`s that
binary once per failing block, which on this repository is four spawns of 102 MB
for three failures rather than one. Neither is a stencil.

### 6.10 The largest unit stops being the ceiling, 2026-09-04

§6.9 left the emission bounded by one unit, and named the two ways past it that
it could see: make a function cheaper, or make a unit smaller. There is a third,
and it is the one that does not touch the build system — **divide the emission
of a unit without dividing the unit**. `design/native/CODEGEN-STENCIL.md` §4.3
is the mechanism; this is the measurement.

**The measurement is on a synthetic, and that is a caveat rather than a
footnote.** §6.9's repository no longer compiles against this toolchain: the
filesystem effect was split into `FsRead` and `FsWrite` in the meantime
(`core/fs`), and nine of its eighteen packages are written against the
un-split one, so the half that still compiles emits about 20 ms in total and
can say nothing about a 1.10 s unit. What replaces it is a program *shaped*
like the finding — one module instantiating `core/ordmap` at two hundred key
and value types, which puts **14,200 monomorphized functions in `core_ordmap`**
against the real repository's 11,267 — and the numbers below should be read as
that shape rather than as that repository.

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
functions, no shared mutable state that is not a memo. So the members are cut
into contiguous parts of 512 and the parts of *every* unit are one flat work
list, with the per-unit assembly — symbols, relocations, writer, digest — left
where it was.

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

**The 1.1% of object bytes is what the division costs and is not free.** Three
things are per-`Jit` and become per-part — the constant pool's deduplication,
the map of a stencil's spilled constants, and the generated glue — and the glue
is the one that shows: two parts that both drop a `[Str]` get a copy each under
different local names. Measured across part sizes from 256 to 2048 the emission
wall is the same to within the noise and the object bytes move about 0.2% per
halving, so 512 is the smallest part that costs about one per cent.

**What the floor is now, and it is a different shape.** 67 ms of the biggest
unit is its assembly: 27 ms in the object writer, 22 ms building a symbol table
and 402,000 relocations, 13 ms concatenating the parts, 5 ms hashing. Every one
of those is one unit's own and serial by construction.

**One finding is recorded and deliberately not acted on.** The `codegen` key
`compile_unit` computes — 164 ms of the 614, a quarter of the biggest unit —
is **thrown away**. `build::actions::codegen_units_for` matches the emitted
object by *name* and keeps the key it was already handed, which
`unit_hashes` computed in parallel above the emission from the same
`render_func` text; the backend's answer is never read. Half of it is recovered
here by rendering in the parts (the text is a concatenation, so the digest is
unchanged), and the other half would need `Backend::emit`'s contract to say
that the key is the caller's — which is a build-system change, and this one was
chosen for leaving the cache story alone. `llvm/mod.rs` computes the same
discarded key the same way.

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
lowering calls are directly attributable to their rows.

**A controlled sweep is the other instrument, and it is not the lesser one.**
The suite's whole parameter space is a command-line flag, so a suspected
superlinear term can be tested by holding everything constant and moving the one
axis it is suspected in. `--param lines_per_module=2500` at a fixed `--scale`
changes the codegen unit count and nothing else that matters, and a row moving
back to its old rate under it is a stronger statement than a flame graph: a
profile says where the time is, and an experiment like that says what the time
is a *function of*. That is how §6.4's Θ(units × functions) finding was made.
What a profile cannot give here is cycles-per-line; the substitute discipline is
to re-run the suite after every change and let §6's table, not the profile, say
whether the change was real.

**Two sweeps beat one.** A sweep over enum width once produced a curve that fell
and then flattened — suggestive, and not an answer. A second sweep held the
width and moved the derive load, and the answer was a step function between one
derived trait and the next. An axis that a single sweep leaves ambiguous is
often two axes, and the second sweep is cheap: both of those together were under
ten minutes at a scale small enough to be quick and large enough to be real.

The two instruments are complements rather than alternatives, and §6.4's first
finding is the case that shows it. The sweep named the axis — the unit count —
and the two suspects it made obvious were two thirds of the cost; the third was
a strongly-connected-components pass inside a constructor, which nothing about
the axis suggested and a profile pointed straight at.

**macOS ships a profiler even where nothing is installed**:
`sample <pid> <seconds> 1 -file out.sample`, whose "sort by top of stack"
section is enough to read a self-time ranking. It is a poor substitute for
`samply` — no inverted call tree worth the name, and Rust's mangled symbols come
out raw — and it is much better than nothing.

