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
whole set costs 17 KB of git, which is the argument for the kind stated as a
number. §4 lists the points and what each moves.

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
  under a second; the anchor — `mixed` at both scales — under a plain
  `--validate`, which is what it covered when there were only two manifests and
  is still ten seconds; the sample under `--validate --set=scale`, twenty-one
  seconds; all forty under `--validate --set=scale-full`, two minutes and
  forty-seven. The rule that does not bend is the last row: **the whole pinned
  half is checkable by one documented command**, and a re-pin is what happens
  when it fails.
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

**Many modules, not one file.** At 100,000 lines the mixed corpus is 389
modules with a real import graph, each module calling into one to three others'
functions *and* naming one of their types. Three reasons, all Carbon's: it stops
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
| `cli/benches/pinned/` | Forty digest-pinned manifests — twenty parameter points at 100k and 1M — and no source. 17 KB. |

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
| `mixed-many-files`, `mixed-few-files` | Module count against module size: 15,640 modules at 1M against 192. |
| `mixed-libs`, `mixed-deep-graph`, `mixed-wide-graph` | Import-graph shape: clustered, deep, wide. |
| `struct-heavy`/`struct-light`, `enum-heavy`, `impl-heavy`, `match-heavy`, `string-heavy`, `list-heavy`, `long-bodies` | Construct-family weight — one kind turned up until it is most of the corpus. |
| `generic-blowup`/`generic-free` | Generics density: 243k monomorphized functions at 1M against 119k. |
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
handed to it — so the seven are chosen to span both: `mixed-many-files` (15,640
units at 1M) and `mixed-few-files` (192) at the ends of the first,
`generic-blowup` (243k monomorphized functions) and `enum-heavy` (59k) at the
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
reason §3.1 gives: 0.3 s under `--quick` and no digests, 10 s plain and the
anchor's two, 21 s under `--set=scale`, and 2 min 47 s under `--set=scale-full`
for all forty. The last one is the command that answers "is every pinned digest
still good", and it is the one to run after touching `generate.rs`.

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
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` — whichever machine
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

| Area | Files | Code | Comment | Blank | Total | Comment share |
|---|---:|---:|---:|---:|---:|---:|
| `parsing` (lexer, parser, tree) | 5 | 5,035 | 1,122 | 415 | 6,572 | 17% |
| `compiler/semantics` | 11 | 8,927 | 1,894 | 592 | 11,413 | 17% |
| `compiler/middle` | 13 | 12,451 | 4,094 | 1,017 | 17,562 | 23% |
| `compiler/backend/js` | 4 | 5,578 | 1,290 | 346 | 7,214 | 18% |
| `compiler/backend/cranelift` | 5 | 5,736 | 2,495 | 403 | 8,634 | 29% |
| `compiler/backend/llvm` | 6 | 7,780 | 3,093 | 452 | 11,325 | 27% |
| `compiler/backend/stencil` | 18 | 12,681 | 4,563 | 789 | 18,033 | 25% |
| `build` | 13 | 8,049 | 2,362 | 651 | 11,062 | 21% |
| `commands` | 12 | 4,207 | 1,378 | 275 | 5,860 | 24% |
| `documentation` | 9 | 5,115 | 1,072 | 443 | 6,630 | 16% |
| `language_server` | 4 | 1,049 | 267 | 114 | 1,430 | 19% |
| shared, driver, stdlib glue | 13 | 5,076 | 2,106 | 518 | 7,700 | 27% |
| **`cli/src` total** | **113** | **81,684** | **25,736** | **6,015** | **113,435** | **23%** |
| `cli/runtime` | 15 | 3,729 | 2,850 | 375 | 6,954 | 41% |
| `cli/tests` | 33 | 14,614 | 5,474 | 1,404 | 21,492 | 25% |
| `cli/benches` | 4 | 3,613 | 1,156 | 269 | 5,038 | 23% |

**This census predates 2026-08-29 and still counts a backend the tree no longer
has.** `compiler/backend/cranelift`'s five files and 8,634 lines are gone, and
so are `cli/tests`'s Cranelift-only suites; every total above still includes
them. The row is left in place rather than subtracted by hand, because "the
compiler got smaller" is a claim somebody checks by re-running the script, and a
table half re-counted is worse than one honestly stale. The next census re-takes
it whole.

And the Buri-language side, which the compiler has to get through:

| | Files | Code | Comment | Blank | Total |
|---|---:|---:|---:|---:|---:|
| standard library (`core/*`, `ui/*`) | 38 | 3,386 | 2,435 | 731 | 6,552 |
| test corpus (`cli/tests/**/*.buri`) | 1,021 | 19,082 | 3,101 | 3,471 | 25,654 |
| shipped documentation (`cli/src/docs/**/*.md`) | 113 | — | — | — | 12,000 |

The three phases the goals name are **35,500 lines of Rust** between them:
`parsing` at 6.6k, `semantics` at 11.4k, and `middle` plus `backend/js` at 24.8k.
That is the surface any optimization wave has to work on. The three native
backends are another 38k on top of it, and they are not what the goals measure —
goal 3 is a lowering rate, and a backend that is twice the code for the same
rate has spent it on something else.

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
> **How the table was assembled, because one command no longer does it.**
> `cargo bench -p buri --bench compiler` cannot complete a default run today: it
> overflows the main thread's stack on `wide-match/10k`, after every `mixed`
> scale and every realistic profile have finished and before that corpus's first
> timer. The abort reproduces identically at the commit before the removal, so
> it is pre-existing rather than a consequence of it, and `--quick` — which runs
> `wide-match` at 1k — is green. The rows below were therefore taken with
> `--only=` selections and individual `--shape=` runs rather than in one
> command. That is a bug of its own, being fixed separately; until it is, nobody
> can take this table in a single invocation.

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

| Phase | Goal | Measured | Gap |
|---|---:|---:|---:|
| lex | (10 M shared) | 12.06 M | **MET** |
| lex+parse | 10 M | 6.40 M | 1.56× |
| sema | 1 M | 1.32 M | **MET** |
| lower+js | 100 k | 311 k | **MET** |
| lower+macos-arm64 | 100 k | 133.3 k | **MET** |

Two of the three goals are met, and the third is now met on **both** lowering
backends rather than on the JavaScript one alone. Lex+parse started at 1.45 M
lines/s and is 4.4× that now; native lowering started at nothing measurable,
because the realistic corpora could not be compiled natively at all.

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
falls back to JavaScript per suite — out loud — where the toolchain or the
suite's program needs it (`commands/test.rs`;
`design/native/ARCHITECTURE.md` §4). The set of hosts on which that fallback
fires got one member wider on 2026-08-29: the dev backend now answers for the
triples it has a stencil library for and no others
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
- **`buri_rt_list_get`**, which is the whole of the matmul kernel's remaining
  gap — 1.56× when that was measured, 1.38× against LLVM `-O2` in §6.2's newer
  series — and is an out-of-line call that bounds-checks and `memmove`s one
  element into an `Option` payload. Open-coding it the way `list.map` already
  open-codes its loop is the next 2× on that shape.
- **The producer half of fusion.** `range` is still materialized.
- **Derived `Show`**, which needs the design decision in §6.4 rather than more
  tuning.
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
- **Lex+parse's last 1.56×.** The plateau without a design change is
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

