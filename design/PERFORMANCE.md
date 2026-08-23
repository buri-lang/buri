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
cache — but about the end-to-end cells (§6.9) and about any harness that asks
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
order of magnitude is a flag rather than an absence; §6.7 is what it found the
first time it was taken.

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
| `cli/benches/corpus.rs` | Saved and pinned corpora: the manifest, the digest, discovery, `--record`, `--pin`, the size cap. |
| `cli/benches/compiler.rs` | The harness: warmup, repetition, median/MAD, the phase timers, the report. |
| `cli/benches/corpora/` | Eight checked-in corpora, 0.55 MB, capped at 2 MiB. |
| `cli/benches/pinned/` | Forty digest-pinned manifests — twenty parameter points at 100k and 1M — and no source. 17 KB. |

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
```

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
rates §6.7 records, one repetition of a 10M native row is about three minutes,
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
> in every one of them — checked, not assumed. Every reading in §6.1 through
> §6.9 was taken at generator revision 1 and corpus revision 1 and is still
> comparable with one taken at revision 2; a rate quoted in lines/s is unmoved
> because the line count is unmoved, and a rate quoted per byte would differ by
> the ratio above.

**2026-08-21: `buri test` defaults to the native dev backend.** A suite that
names no platform is compiled with Cranelift and run as a binary, and falls back
to JavaScript per suite — out loud — where the toolchain or the suite's program
needs it (`commands/test.rs`; `design/native/ARCHITECTURE.md` §4). The number
that paid for the change is the incremental one: a one-line edit at 104k lines
is 502 ms to verdict native against bun's 622 on the fast suite and 1,484
against 1,742 on the compute suite, the first measurement in this project where
the native compile column is itself the faster one.

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

### 6.5 Round three: the declaration unpin, and the backend question settled, 2026-08-19

The last owned strings left the tree: `Ident` became a 12-byte span-based
`Name`, `TypeExpr` a fourth arena. The one test that had pinned both
(`standard_library.rs`) turned out to pin only its own accessor mechanics —
two lines changed, policy and assertions byte-identical.

| Corpus | Phase | Lines/s | Gap | Movement |
|---|---|---:|---:|---|
| mixed/100k | lex+parse | **5.92 M** | **1.7×** | 1.45 M at baseline — 4.1× total |
| mixed/100k | sema | 1.00 M | 1.0× | holds |
| mixed/100k | lower+js | 279 k | 0.36× | holds |
| mixed/100k | lower+native ×3 | 51–55 k | 1.8–2.0× | holds |

Allocations per token through lex+parse: 0.88 at baseline → **0.18**. What
remains of goal 1 is C3 plus ~7% of declaration `Vec`s and genuinely-owned
text.

**The dev-backend question is settled by measurement.** LLVM at `-O0` is
2.1–4.9× *slower* than Cranelift at 10k lines on the shapes that decide it,
and slowest exactly where Cranelift already misses — switching would take
`enum-heavy` from 21 k to 4 k lines/s. LLVM at `-O2` is 2.4–15× slower.
Cranelift stays the Debug backend; LLVM is now wired as the Release backend
(`select` maps native Release to it under `backend-llvm`, 759 tests green
with the feature, outputs byte-identical to Cranelift and JS on real
programs). Open against it: LLVM lacks the nine open-coded list loops, so
the realistic release rows skip; and Cranelift's `wide-match` has a
linux-x86_64-only superlinear cliff (159 k → 25 k, 1k → 10k) that arm64
does not share.

**The dev-loop question was settled the other way.** Erasing generics in a
dev profile (2–10× measured runtime cost, a second value model through both
backends) and moving instantiation placement (worth exactly one unit of
blast radius, and weak symbols would cost the direct branches) were both
refuted by measurement: a one-line edit at 118 k lines costs 2,622 ms, of
which 64% is `Backend::emit` re-emitting 364 already-cached units for want
of a per-unit parameter, and the 362-of-365-unit invalidation is the IR
renderer printing callees as dense global indices — three lines from being
2 of 365. Monomorphization itself is 41 ms of the loop. The fix wave
(symbol-keyed rendering, per-unit emit, derives out of the root unit) is
~60 lines against a projected 2,622 → ~940 ms.

### 6.6 Round four: the platform cliff, the honest cache, and the fuzz net, 2026-08-19

Three waves closed the round. The incremental-build fixes (symbol-keyed IR
rendering, per-unit `Backend::emit`, derives out of the root unit) took a
one-line edit at 117k lines from **2,434 ms to 949 ms** and the blast radius
from 37 of 41 units to 1 — and found that index-keyed rendering had been a
latent *miscompile* (a stale cached object could survive a callee rename into
an undefined-symbol link). The backend wave killed the linux-x86_64
`wide-match` cliff — a regalloc2 bundle-merge re-sort that x86-64's
two-address `Reuse` constraint always triggered and three-address AArch64
never did — taking that row **26 k → 308 k lines/s** and arm64 to 319 k with
target-neutral emission changes. And two new test binaries joined the suite:
`failing` (84 pinned failure reports — the runner's failure path held to the
success path's standard) and `fuzz` (six oracles, nine searches, 8 s
deterministic in CI, soak mode unbounded), bringing the suite to **718
tests**. The soak's seven minimized findings sit in `cli/tests/fuzz/` as
`OPEN` cases that flip red when fixed; none is a crash, hang, miscompile or
nondeterminism — 1.7 M inputs, 557 k parameter points and 2,547 differential
programs found the front end and all three backends sound.

**Where every goal stands (mixed/100k, authoritative run):**

| Phase | Goal | Measured | Gap |
|---|---:|---:|---:|
| lex | (10 M shared) | 11.3 M | **MET** |
| lex+parse | 10 M | 6.10 M | 1.64× |
| sema | 1 M | 1.04 M | **MET** |
| lower+js | 100 k | 271 k | **MET** |
| lower+native (3 triples) | 100 k | 51–54 k | 1.85–1.95× |

**The dev/release configuration question, settled by measurement:**

- **Dev: Cranelift, whole-binary link, per-unit emit.** LLVM at `-O0` is
  2.1–4.9× slower to lower and worst exactly where Cranelift is weakest;
  erasure and placement changes were refuted (§6.5). The incremental loop is
  949 ms at 117 k lines — codegen now O(changed units), the residual split
  51% whole-program analysis / 19% link / 18% process / 12% codegen.
- **Release: LLVM at O2**, wired and verified byte-identical to the other
  backends across 27 programs. It lowers at 3.9–6.8 k lines/s — 15–26× under
  goal 3 — which is the price of LLVM's optimizer, not of this repository's
  lowering; the goal is met by the dev path a developer actually iterates on.

What remains open, in the order it matters: realistic native lowering at
~1.9× (88% inside Cranelift's `define_function` — a value-model or codegen-
strategy question, not a loop to tighten); `enum-heavy` native at ~5×;
lex+parse's last 1.64× (C3 plus ~7% of declaration `Vec`s); the seven fuzz
findings (one is user-visible data loss: `buri format` halves the
backslashes in an import path on every run) and the five failure-report
bugs, all pinned by their suites.

### 6.7 The scale tier, and the first memory numbers, 2026-08-20

The fourth order of magnitude, taken for the first time. `--set=scale` runs two
digest-pinned corpora — `mixed` at 100,755 lines and at 1,007,259 lines, 3,590
modules and 132,396 monomorphized functions — through the same phase seams as
every other row. The 1M rows are under §2's documented deviation (≥3
repetitions), and their dispersion is 0.2–0.4%, which is tighter than the
default table's: at this size a repetition is long enough that the machine's
noise averages out inside it.

| Corpus | Phase | Lines/s | Gap | ±MAD | vs 100k |
|---|---|---:|---:|---:|---:|
| pinned/100k | lex | 11.17 M | **MET** | 1.2% | — |
| pinned/1M | lex | 11.04 M | **MET** | 0.1% | **−1.2%** |
| pinned/100k | lex+parse | 6.12 M | 1.6× | 0.5% | — |
| pinned/1M | lex+parse | 6.01 M | 1.7× | 0.2% | **−1.8%** |
| pinned/100k | sema | 1.01 M | **MET** | 1.0% | — |
| pinned/1M | sema | 925 k | 1.08× | 1.4% | −8.4% |
| pinned/100k | lower+js | 288 k | **MET** | 1.0% | — |
| pinned/1M | lower+js | 264 k | **MET** | 0.0% | −8.6% |
| pinned/100k | lower+macos-arm64 | 55.0 k | 1.8× | 1.1% | — |
| pinned/1M | lower+macos-arm64 | **31.6 k** | **3.2×** | 0.6% | **−43%** |

Two independent runs of the tier were taken; the second is the table. Between
them the absolute rates move by up to 3% and the *deltas* by a few points —
lexing and parsing land between −2% and +2%, sema between −5% and −8%, JS
lowering between −6% and −9% — and the native delta is −42% in both, to the
tenth of a percent. That stability is what makes the last row a finding rather
than a bad afternoon.

**Four of the five phases are flat across the decade.** Lexing and parsing do
not notice the extra order of magnitude at all. Sema and JavaScript lowering
give back 5–9%, which is inside the band a decade of scale can be expected to
cost on a memory hierarchy and outside the band worth acting on — but they are
the rows to re-read when the tier is next run, because 8% is not nothing and
both of them are within a hair of their goal. The pinned 100k row agrees with
§6.6's generated 100k row throughout, which is the §3.1 pairing check passing on
its third leg.

**Native lowering is the one row that falls, and it falls by 43%.** The cause is
named below, and it has since been fixed: the row now reads **56.1 k lines/s**,
and the table above is the last reading taken before the fix. What follows is
the evidence in the order it was taken, because the method that found the axis
is worth more than the number it produced. The suspect is not the codegen:

`--split` puts the whole of the loss inside `Backend::emit`. Per line, across
the decade: monomorphization +3%, `middle::run` +9%, `middle::native` +14%,
`lower::run` +22% — and `emit` **+86%**. Everything above the backend is
essentially linear.

The shape of the curve says quadratic rather than cache: `mixed` native
lowering runs at 58.5 k lines/s at 30k, 52.4 k at 100k, 46.4 k at 300k and
30.2 k at 1M — the first run's readings at the two ends, so that the four
points are one series — which fits `16.5 + 0.0165·(lines/1000)` nanoseconds per
line to within a few percent at every point. That is a term which grows
linearly *per line*, which is to say quadratically in total.

And a controlled experiment names the axis. Hold the line count and the
function count fixed and cut the *codegen unit* count tenfold —
`--param lines_per_module=2500`, so 1M lines is ~400 units instead of 3,590 —
and native lowering goes from 30.2 k to **52.2 k lines/s**, which is the 100k
rate to within noise. `emit` alone falls 28.6 s → 16.1 s. The rate is a
function of the unit count, not of the program size.

That points at the two whole-program scans the Cranelift backend performs **per
unit**:

- `emit::Unit::new` (`backend/cranelift/emit.rs`) walks all of
  `program.funcs` to collect the unit's own functions, and allocates a
  `vec![None; program.funcs.len()]` linkage table beside it. Both are per unit.
- `compile_unit` (`backend/cranelift/mod.rs`) walks all of `program.funcs`
  again, filtering by unit, to build the text whose hash is the unit's
  `codegen` cache key.

Both are Θ(units × functions). At 100k that is 360 × 13,162 = 4.7 M steps and
nobody notices; at 1M it is 3,590 × 132,396 = 475 M — a hundred times the work
for ten times the program — over an array too large to stay in any cache, which
is why the constant is large as well as the growth.

#### The fix, and the third scan the experiment did not name

`ir::Program::funcs_by_unit` buckets every function index by the unit that owns
it, in one pass, and each unit is handed its own row. Both scans above then read
a list of about thirty-seven entries rather than 132,396: `Unit::new` filters
the row for the functions that have bodies, and the cache key concatenates the
row. The linkage table becomes a map keyed on the function index instead of a
slot per function in the program — a unit declares its own functions and the
handful it calls across a boundary, so the array was an allocation and a memset
per unit for a row that was almost entirely empty.

**The cache key does not move, and that is a property of the row rather than a
hope.** A row is ascending in function index, which is the order the discarded
filter yielded, so the bytes hashed are the same bytes. It was checked as well
as argued: a temporary assertion inside both backends' key computation compared
the new text against the old filter's text for every unit of every program the
two test suites compile, and a repository built by the pre-change binary is
served entirely from its cache by the post-change one — 41 units, 41 hits, no
misses.

That bought back a third of the loss and no more, and a sampling profile found
the rest, which the unit-count experiment had pointed at without naming:
`Abi::new` is per unit, and building the `middle::layout::Layouts` inside it
walks **every type constructor in the program** and runs a strongly-connected
components pass over them, to decide which types are recursive together. That
answer is a function of the checker's tables and of nothing else, so it is now a
`layout::Cycles` built once and shared by every unit's memo table. Which is the
general shape of all three: a per-unit object whose *construction* was a
whole-program question.

The LLVM backend had all three and a fourth, `emit::observe` — a whole-program
fixpoint over every call in the program, rebuilt per unit — plus a memo table
sized by the program's interned types. All four are hoisted or keyed there too,
and its `codegen` key is stable by the same argument and the same check.

| lines | units | before | after |
|---|---:|---:|---:|
| 30k | ~110 | 58.5 k | 60.4 k |
| 100k | 360 | 54.8 k | 57.8 k |
| 300k | ~1,080 | 46.4 k | 55.9 k |
| 1M | 3,590 | **31.4 k** | **56.1 k** |

**The rate is no longer a function of the program's size.** `16.5 + 0.0165·
(lines/1000)` nanoseconds per line has become a constant ~17.5 ns/line across a
33× range, and the gap to the 100,000 lines/s goal is 1.8× at every scale
instead of 3.2× at the top. `emit` at 1M falls 32.1 s → 17.9 s. The 100k and
1M readings are the pinned corpora; the 30k and 300k readings are the generated
`mixed` shape at those scales, which is how the four-point series was taken
before the fix as well.

Worth saying plainly: this was a *build system* cost, not a language one. It was
invisible to `buri build` on a warm cache — the incremental loop of §6.6 emits
one unit — and it landed squarely on a clean build of a large program. That the
warm loop is untouched was confirmed rather than assumed: a leaf-body edit
re-emits one unit of forty-one before and after, at the same wall time.

#### Peak memory, the first reading

§1 calls peak memory the obvious fourth column and does not have a goal for it.
Here is the first data, from `--set=scale --rss`, as the peak resident set size
of a process that stopped after the named phase (§4):

| Phase | 100k peak | B/line | 1M peak | B/line |
|---|---:|---:|---:|---:|
| corpus in memory | 11.6 MB | 121 | 104.5 MB | 109 |
| lex | 28.1 MB | 293 | 236.8 MB | 246 |
| lex+parse | 39.0 MB | 405 | 344.9 MB | 359 |
| sema | 128.2 MB | 1,334 | 1,227.3 MB | 1,278 |
| lower+js | 272.4 MB | 2,835 | 2,697.1 MB | 2,808 |
| lower+macos-arm64 | 298.4 MB | 3,105 | 2,788.9 MB | 2,903 |

**Memory is linear across the decade, to within a few percent per line, and
every one of those percent points is in the cheap direction.** Semantic
analysis costs about 1.3 KB per line at both scales; the whole compilation
peaks at about 2.9 KB per line. The 100k and 1M columns agree more closely than
any of the *time* columns do. Three things worth writing down before there is a
goal to hold them to:

1. **Compiling a million lines takes 2.8 GB.** A laptop can hold that and a CI
   container often cannot, and it is the honest reason a memory goal will
   eventually be needed — not because anything here is superlinear, but because
   2.9 KB per line is a *choice* nobody has argued about yet.
2. **The parse tree is ~360–400 B/line and the checker's output is ~1,300.**
   The representation waves of §6.3 and §6.4 moved the first number; the second
   is three times larger and has never been optimized for size at all.
3. **Peak RSS varies by a few percent run to run**, because it is the
   allocator's high-water mark and not a count. It is a two-significant-figure
   measurement and should be quoted as one.

And the negative result, which is the useful half: the native cliff above is
**not** a memory blowup. Peak RSS per line is flat — very slightly *lower* at
1M — so whatever `emit` is doing wrong, it is doing it in time and not in
space.

### 6.8 The parameter sweep at scale, 2026-08-20

§6.7 asked one profile at two scales. This is twenty parameter points at the
same two, which is a different question: not "does the rate hold as the program
grows" but "which parameter is the rate a function of". Both tiers came off one
build, so the 100k/1M comparison is a comparison of sizes. The 100k tier was run
twice and agrees with itself within ±8% on all but three rows; those three were
taken while another process was compiling, and are named as such rather than
smoothed.

Rates in lines/s, macOS aarch64, ten cores, generator revision 1, corpus
revision 1. The two right-hand columns are the ones worth reading: **Δ100k→1M**
is what a decade of program size costs this point, and **Δvs `mixed`** is what
the parameter costs at a million lines.

| Point | sema 1M | Δ100k→1M | Δvs `mixed` | js 1M | Δ100k→1M | Δvs `mixed` | native 1M | Δ100k→1M | Δvs `mixed` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `mixed` | 901 k | −13% | — | 252 k | −7% | — | 52.2 k | −8% | — |
| `mixed-many-files` | 1.14 M | −9% | +27% | 402 k | −1% | +59% | 89.5 k | −5% | +71% |
| `mixed-few-files` | 821 k | −3% | −9% | 240 k | −3% | −5% | 56.5 k | −3% | +8% |
| `mixed-libs` | 915 k | −9% | +2% | 251 k | −10% | −0% | — | — | — |
| `mixed-deep-graph` | 901 k | −13% | +0% | 248 k | −13% | −2% | — | — | — |
| `mixed-wide-graph` | 899 k | −13% | −0% | 260 k | −8% | +3% | — | — | — |
| `struct-heavy` | 1.39 M | −7% | +54% | 533 k | −2% | +111% | 83.6 k | −4% | +60% |
| `struct-light` | 863 k | −7% | −4% | 239 k | −5% | −5% | — | — | — |
| `enum-heavy` | 942 k | −11% | +5% | 193 k | −9% | −23% | **16.8 k** | −16% | **−68%** |
| `generic-blowup` | **677 k** | **−29%** | −25% | 177 k | −13% | −30% | 35.2 k | −20% | −33% |
| `generic-free` | 886 k | −16% | −2% | 264 k | −13% | +5% | — | — | — |
| `derive-heavy` | 933 k | −11% | +4% | 286 k | −4% | +14% | 47.0 k | −6% | −10% |
| `impl-heavy` | 893 k | −20% | −1% | 282 k | −10% | +12% | — | — | — |
| `match-heavy` | 926 k | −14% | +3% | 242 k | −7% | −4% | — | — | — |
| `string-heavy` | 910 k | −6% | +1% | 264 k | −4% | +5% | — | — | — |
| `list-heavy` | 879 k | −13% | −2% | 267 k | −11% | +6% | — | — | — |
| `long-bodies` | 1.06 M | −16% | +18% | 293 k | −10% | +16% | — | — | — |
| `comment-heavy` | 1.67 M | −6% | +85% | 463 k | −6% | +84% | — | — | — |
| `comment-free` | 719 k | −6% | −20% | 202 k | −5% | −20% | — | — | — |
| `long-idents` | 871 k | −14% | −3% | 247 k | −10% | −2% | — | — | — |

Lexing and parsing are omitted from the table because they have nothing to say:
every point lexes between 7.8 and 12.4 M lines/s and parses between 5.2 and 6.6,
and no point loses more than 7% across the decade except `mixed-few-files`
(−21%), whose modules are 180 KB each at 1M.

**Nothing new is superlinear, and that is the headline.** Twenty points, four
phases, a tenfold change in size: the worst per-decade loss anywhere is −29%,
and the median is −8%. §6.7 found a genuine quadratic by measuring one profile
at two scales; twenty profiles at the same two scales find no second one. The
sampling that would have hidden a quadratic — three repetitions instead of ten —
is not what is holding these numbers together: the dispersions are under 2% on
all but four rows.

**The rate is a function of declarations per line before it is a function of
anything else.** `comment-heavy` "checks" at 1.67 M lines/s and `comment-free`
at 719 k, and the checker is identical in both. §2 counts a comment as a line on
purpose, and the 2.3× between those two rows is the price of that honesty stated
as a number. The same reading applies to `struct-heavy` (+54% on sema) and
`long-bodies` (+18%): a corpus whose lines are mostly field declarations or
`let` bindings has fewer things to resolve per line than one whose lines are
mostly signatures. **Any goal quoted in lines/s is quoted against a corpus's
comment and declaration density, and the twenty-point spread — 719 k to 1.67 M
on the same phase — is the size of that dependence.** The suite already prints a
token rate beside the line rate for lexing and parsing, for exactly this reason;
this is the strongest argument it has produced for wanting one beside every
phase, and for a goal that is not quoted against a single profile's density
without saying so.

**The two axes the native backend is a function of behave.** Unit count:
`mixed-many-files` at 1M is 15,640 codegen units and runs at 89.5 k lines/s,
**71% faster** than the anchor's 3,590 units — the opposite sign from before
§6.7's fix, and the cleanest possible confirmation that the fix held at ten
times the scale it was found at. Its per-decade loss is −5%. IR size:
`generic-blowup` at 243 k monomorphized functions runs 33% below the anchor at
1M and loses 20% across the decade, `generic-free` at 119 k loses none of it.
Neither is a cliff.

#### The outlier: a derived `Show` on a wide enum

One row is more than a factor off everything around it. `enum-heavy` lowers
natively at **16.8 k lines/s at 1M and 20.1 k at 100k**, against the anchor's
52.2 k and 56.7 k — 3.1× slower, and **6× off goal 3**, the largest gap any row
in this document has ever recorded. It is not the JavaScript backend's problem:
the same corpus emits JS only 23% below the anchor.

It is not a function count either, which is what makes it interesting.
`enum-heavy` at 100k has 5,903 monomorphized functions against `mixed`'s 13,162
— *fewer than half* — and `--split` puts the whole difference in `emit`:

| 100k, `lower+macos-arm64` | mono | middle-A | middle-native | lower(IR) | emit |
|---|---:|---:|---:|---:|---:|
| `mixed` | 99.5 ms | 64.2 | 47.7 | 87.0 | 1,453 ms |
| `struct-heavy` | 30.8 ms | 48.9 | 28.5 | 47.2 | 945 ms |
| `enum-heavy` | 35.7 ms | 76.0 | 140.9 | 149.0 | **4,703 ms** |

0.80 ms per monomorphized function against the anchor's 0.11.

Two controlled sweeps at a fixed 30,000 lines name the cause exactly, and the
second one names it to a single trait. Moving the enum width alone:

| `variants_per_enum` | native |
|---|---:|
| `3..7` | 39.7 k |
| `6..12` | 28.3 k |
| `12..24` (the pinned point) | 16.5 k |
| `24..48` | 16.7 k |

Then holding the width at the pinned `12..24` and moving the derive load:

| `derives` | traits derived | native |
|---|---|---:|
| 0 | none | 96.6 k |
| 1 | `Eq` | 97.9 k |
| **2** | `Eq, Show` | **20.8 k** |
| 4 | `+ Ord, Hash` | 20.9 k |
| 6 | `+ ToJson, FromJson` | 20.2 k |

**The whole of it is the second derive, and the four after it are free.** A
derived `Show` on a wide enum costs 4.7× the entire native lowering row;
`Eq`, `Ord`, `Hash`, `ToJson` and `FromJson` together cost nothing measurable.
That also explains why the width sweep flattens rather than continuing to fall:
each enum costs a fixed dozen lines of surrounding functions whatever its width,
so at a fixed line count a wider enum packs more variants into the same lines —
until the fixed part stops mattering and the packing stops changing. A per-line
rate falling under a per-variant cost that is constant is what that looks like.
**It is not superlinear in anything.** It is a large constant, per variant, on
one derived trait.

##### The suspects, named

`cli/src/compiler/middle/derives.rs:1454`, the `Desc::Enum` arm of `show`: one
match arm per variant, and inside each arm a string template with a literal
piece per field and a recursive `Show` call per field. The generated body is
Θ(variants × fields) with a large constant, and there is one per instantiated
type. Turning `Show` on doubles the emitted JavaScript for this corpus — 367 KB
to 685 KB at 30k, at an unchanged 1,790 monomorphized functions — so the
expansion is real and it is big.

But the expansion is not the cost. `--split` across the same two derive
settings:

| 30k, `derives` | middle-native | lower(IR) | emit |
|---|---:|---:|---:|
| 1 (`Eq`) | 9.9 ms | 13.2 | 277 ms |
| 2 (`Eq, Show`) | 38.7 ms | 44.8 | **1,498 ms** |

Generating the expansion costs 29 ms. Lowering it to IR costs 32 ms more.
**Emitting it costs 1,221 ms more** — forty times the expansion that caused it,
and 3.85 ms per KB of generated code against the ~1.1 ms/KB the anchor's
ordinary code costs. So the second suspect is the backend's handling of these
bodies specifically: a `Template` of concatenations, one per arm, in a function
with two dozen arms. Whether that is the string-building lowering, the block
count, or register pressure across a wide switch is not something this
measurement can distinguish, and it is where a sampling profile should start.

**Not fixed in this wave, per instruction.** The shape of a fix is a choice
between two things and should be argued rather than assumed: either the derived
`show` for an enum stops being one inlined body — a per-variant helper, or a
table-driven renderer that the backend emits once — or the backend gets cheaper
on concatenation-heavy bodies. The first is a `middle::derives` change and the
second is a backend one, and the numbers above do not yet say which is right.

#### Two rows to watch, below the threshold

`generic-blowup` loses **29% of its semantic-analysis rate across the decade**,
which is more than twice the anchor's −13% and the largest per-decade loss in
the table. It also has the table's worst dispersion (±14% MAD over three
repetitions), so it is a candidate for a re-read before it is a candidate for an
investigation — but generics density is the one axis where the checker has a
plausible superlinear term, and this is the point built to find it.

`mixed-few-files` lexes 21% slower at 1M than at 100k, and 26% slower than the
anchor at 1M, on modules of 180 KB each. Every other point is within 7%. That
one is a cache reading rather than an algorithmic one and it is the reason the
point exists.

### 6.9 The consolidated dev-mode round, 2026-08-20

Everything above §6.8 measures the *compile* column. This section measures the
whole loop — compile plus runtime, per strategy — and it exists because a wave
of runtime work (§6.9.2) invalidated every cross-strategy number the project
had. Those numbers had been taken in four separate sessions on four separate
trees; this round re-took all of them **on one machine, in one session, against
one build**, with every strategy interleaved inside every repetition so a drift
in machine load lands on all of them. Medians of five for the whole-loop cells,
of seven for the kernel runtimes.

Two columns are carried forward rather than re-measured, and are marked `†`:
the **tree-walking interpreter** and the **Cranelift JIT**. Neither strategy
changed, both are scratch prototypes rather than toolchain paths, and re-wiring
them would have measured the prototypes rather than the question. The
interpreter's runtime is genuinely unmoved — it never ran `middle::native`, so
none of §6.9.2 reaches it. The Cranelift JIT's runtime is by construction the
native one, so its compute cells move with the native column and are recomputed
rather than quoted; they are marked derived.

#### 6.9.1 The whole loop, every strategy

Workloads, unchanged from the strategy comparison that named them: a **fast**
suite of 401 tests of two integer assertions each, and a **compute** suite of
four kernels — prime counting below 4,000,000 by trial division (K1),
n-queens 12 (K2), a 320² dense float matrix multiply (K3), and a
`range`/`map`/`filter`/`fold` pipeline over 40,000 × 2,000 (K4). Codebases: a
generated repository at 12,383 lines / 36 modules and the same replicated
tenfold at 103,967 lines / 360 modules, 377 codegen units. *Incremental* is one
line appended to a leaf module with a nonce never compiled before.

**Time to verdict, 104k lines, one-line edit, milliseconds. Bold = the winner.**

| | native AOT | JS (bun) | copy-and-patch | interpreter† | Cranelift JIT† |
|---|---:|---:|---:|---:|---:|
| compile | 1,175 | 528 | 643–653 | 313–319 | ~2,223–2,285 |
| runtime, fast suite | 16.5 | 86.9 | 0.1 | 17.3 | ≡ 16.5 |
| runtime, four kernels | 968.5 | 1,159.5 | 2,071 | 108,864 | ≡ 968.5 |
| **wall, fast suite** | 1,191 | 615 | 719 *(656)* | **~331** | ~2,300 |
| **wall, compute** | 2,144 | **1,678** | 2,780 *(2,717)* | ~109,177 | ~3,253 |
| noop (only the first two have a cross-session cache) | **28** | **28** | 719 *(656)* | ~331 | ~2,300 |

Copy-and-patch's parenthesised figures are with the stencil library preloaded.
The 63 ms it otherwise pays is this prototype's own Mach-O parser, not the
technique, and a shipped implementation pays zero; both are printed because
only one of them is a measurement of copy-and-patch.

**Time to verdict, 12k lines, one-line edit:**

| | native AOT | JS (bun) | copy-and-patch | interpreter† | Cranelift JIT† |
|---|---:|---:|---:|---:|---:|
| compile | 451 | 71 | 55–72 | 27–36 | ~205–260 |
| runtime, fast suite | 5.8 | 35.5 | 0.1 | 17.3 | ≡ 5.8 |
| runtime, four kernels | 957.6 | 1,100.0 | 2,073 | 108,864 | ≡ 957.6 |
| **wall, fast suite** | 457 | 106 | 139 *(75)* | **~48** | ~266 |
| **wall, compute** | 1,414 | **1,148** | 2,196 *(2,131)* | ~108,891 | ~1,190 |
| noop (only the first two have a cross-session cache) | **7** | **7** | 139 *(75)* | ~48 | ~266 |

**Cold, for the two columns whose cold and incremental cells differ** (the other
three have no cross-session cache, so cold ≡ incremental for them):

| | native AOT, 12k | JS, 12k | native AOT, 104k | JS, 104k |
|---|---:|---:|---:|---:|
| fast, wall | 661 | 109 | 2,878 | 619 |
| compute, wall | 1,568 | 1,157 | 3,798 | 1,679 |

**What moved, and it is one thing.** The native compile column is where §6.7's
and §6.8's work left it — 1,175 ms incremental at 104k against 1,199 the last
time it was taken, 528 for JavaScript against 528. The whole of the change is
the **runtime** column: the four kernels went from 2,567 ms to **968.5**, which
takes the native path's compute cell from 3,766 ms to 2,144 and its gap to the
JavaScript incumbent from **2.26× to 1.28×**. On the fast suite nothing moved at
all, because 401 tests of two assertions do not execute anything.

#### 6.9.2 What the runtime wave did — FP wave 1

Four measured optimizations on the native dev path, each with its own
verification. The whole of the change above is these four.

| item | what | where |
|---|---|---|
| 1 | **cheap enum discrimination** — a switch of ≤ 4 cases becomes an `icmp_imm`/`brif` chain instead of a `br_table` with a Spectre `csdb` and an indirect branch, and a discriminant that was just computed is read from its register instead of from memory | `backend/cranelift/emit.rs` |
| 2 | **list-combinator fusion** — `fold ∘ map`, `fold ∘ filter`, `map ∘ map`, `filter ∘ filter`, `count`/`any`/`all` over either, `len ∘ filter` | `middle/fuse.rs`, new |
| 3 | **devirtualizing the known callee** — a call through a closure whose construction this body can see becomes a call by name | `backend/cranelift/emit.rs` |
| 4 | **one shared string joiner per arity** in derived `Show` instead of a concatenation chain per rendered variant | `middle/derives.rs` |

**Item 1 was the largest, and it was not on anybody's list.** Matching *any*
enum of any arity cost **12–15 ns**, against 0.15 ns for an `if` on a `Bool` —
a payload-free two-variant enum cost 13.6 ns, an `Option<Int>` 17.4, and a
niche-packed `Option<Str>` 17.5, all agreeing to within 30% across every
variation of payload, arity and representation, which is what says the cost is
the *match* rather than anything it matches on. It is now **~0.2 ns**. Because
`list.get` returns `Option<T>`, this sat on the inner loop of two of the four
kernels: `list.get` fell from 15.0 ns to **3.9 ns**, and K3 — 65.5 M gets — fell
with it.

| micro-benchmark, 80 M iterations | before | after |
|---|---:|---:|
| payload-free two-variant enum, built and matched | 1,087.5 ms | **107.0** |
| `Option<Int>` returned and matched | 1,393.4 | **172.1** |
| `Option<Str>`, niche representation | 1,400.8 | **215.9** |
| four-variant enum with payload | 1,227.3 | **275.3** |
| 80 M `xs.get(i)` + match | 1,299.5 | **409.7** |
| the same arithmetic with no list | 101.1 | 97.4 |

Matching a payload-free enum now costs about what an `if` on a `Bool` costs:
107.0 ms against 119.4 for a function that returns a plain `Int`.

**The defect was dev-mode-only, and that was checked rather than assumed.** The
same two shapes built as `main`-bearing binaries and run at LLVM `-O2` cost
55.1 ms and 54.4 ms over 80 M iterations — 0.7 ns an iteration in total, which
cannot contain a 12–15 ns match — and `otool -tv` finds no `csdb` and no
jump-table load in either. `mem2reg` sees the tag's store-to-load pair and
`SimplifyCFG` sees a two-destination switch, so the release path never had the
defect. The battleground is the dev path, so it still mattered; what follows
from it is that the *middle-end* half of the same idea (folding a match whose
tag is statically known) is the part with long-run value, because it removes
matches rather than cheapening them on one backend.

**Item 2 runs on the native branch only, and that is a testing decision rather
than a caution.** `cli/tests/native/agreement.rs` compares the JavaScript
artifact's answers against both native ones, and that comparison is the only
mechanical oracle this rewrite has. Fusing in the shared middle would fuse both
sides identically, and **a differential test whose two sides share the
transformation under test proves nothing about it**. Keeping the pass
native-only makes JavaScript the reference implementation of every pipeline in
the corpus, which the fuzz `output` oracle and the 243-block conformance corpus
then exercise for free. The cost — JavaScript does not get faster — is the
smaller loss: V8 allocates the intermediate array with a bump pointer in a
generational nursery, which is a much cheaper machine than `malloc` plus a copy,
so the same rewrite is worth less there than the oracle is worth here.

The pass composes rather than fusing loops: `fold(map(xs, f), g, z)` becomes
`fold(xs, |a, x| g(a, f(x)), z)`, which is the same combinator over a different
list with a bigger lambda — **no new IR node, no new intrinsic key and no
backend change at all**. Only the context-free combinators fuse, and the effect
argument is the language's: SPEC 10.6 forbids a lambda from capturing an
effect-carrying value, so a step is a pure function of its element and
interleaving pure steps is unobservable. The one residual divergence is which of
two *diverging* steps aborts first, which no terminating program can observe;
it is the standard caveat of shortcut fusion and it is in the pass header.

| pipeline shape | before | after | mallocs | bytes |
|---|---:|---:|---|---|
| `map \| filter \| fold` (= K4) | 335.7 ms | **139.9** | 6,205 → **205** | 1.83 GB → **334 KB** |
| `map \| map \| fold` | 353.2 | **130.2** | 4,205 → 205 | 1.28 GB → 334 KB |
| `range \| map \| filter \| len` | 287.5 | **115.2** | 8,204 → 2,204 | 2.43 GB → 640 MB |

The first two now equal their hand-fused forms to within the noise. The third
keeps one list because the *producer* — `range` — is not fused, and that is the
obvious next increment: the hand-written form with no list at all is 58.5 ms.

**Item 3 was fixed one level down from where it was diagnosed, and the
correction is worth recording.** The named site was a `CallValue` whose callee is
syntactically a `FnRef`. That is not where the thunks came from: after
`closures::run` a capture-free lambda is an `ExprKind::FnRef`, which lowers to
`MakeClosure{env: None}`, and the call is then made by the backend's own
open-coded `list.*` loop rather than by `CallIndirect` at all. Fixing only the
named site would have moved nothing. The fix is in `Lower::direct_callee`, where
it catches both paths, under three conditions that are each one clause of the
thunk it replaces: no environment, no borrowed counted parameter, and flattened
arguments and results that are exactly the callee's — the last of which is what
refuses a merged tail-recursive SCC, whose signature carries a dispatch
discriminant no call site supplies. K4 went 559.5 → **335.9 ms** on this item
alone, and a fold over a *named top-level function* — the one-line proof that
passing a function by name got you no closer to a direct call — went 239.9 →
**140.6**.

**Item 4 improved its row by 13% and did not meet its target**, and what it ruled
out is worth more than the 13%. `enum-heavy`'s native lowering went from 20.0 k
to **22.6 k lines/s**, against §6.8's target of bringing `derives=2` within
~1.2× of `derives=1`; the gap went from 4.7× to 4.3×. Three things are now
known, and they eliminate two of the three candidate fixes §6.8 named:

1. **It is not superlinear in function size, so a per-variant helper split will
   not work.** At a *fixed* 512 total variants, regrouping from 128 enums × 4
   variants to 8 × 64 costs the same to within 3%.
2. **The cost is a constant per rendered *field*, not per variant.** At 512
   variants, payload-free derived `Show` is free (231.0 ms against 234.9 with no
   derive at all); one field costs 33 ms, two 72, four 163 — about **0.07 ms per
   hole**.
3. **65% of the row is `regalloc2`.** `regalloc2::ion::Env::init` alone is 18% of
   the whole process, with `BTreeMap<LiveRangeKey>` traffic behind it. The lever
   is CLIF volume and live ranges per hole; the joiner cut the first and slightly
   raised the second.

So the two routes that remain are a variadic join that takes its parts **through
memory** (three dead stores per hole instead of three live leaves, which needs a
runtime entry and both backends), or a descriptor-driven renderer emitted once,
which is what the JavaScript backend already does and is a design decision about
`derives.rs`'s premise rather than an optimization. **It needs a decision, not
more tuning.**

#### 6.9.3 The four kernels: dev, release, and bun

Medians of seven, interleaved, each kernel built as its own `main`-bearing
binary from byte-identical source and executed directly. bun 1.2.13, net of its
own 18.3 ms empty-module floor measured in the same interleaving.

| kernel | native dev (Cranelift) | **release (LLVM `-O2`)** | bun, net | dev ÷ bun | **release ÷ dev** |
|---|---:|---:|---:|---:|---:|
| K1 primes | 239.5 ms | 243.4 ms | 285.4 | **0.84×** | **1.02× — slower** |
| K2 n-queens 12 | 314.7 | **239.6** | 264.6 | 1.19× | 0.76× |
| K3 matmul 320² | 273.8 | **220.8** | 175.4 | 1.56× | 0.81× |
| K4 pipeline | 140.3 | **75.3** | 338.8 | **0.41×** | 0.54× |
| **total** | **968.3** | **779.1** | **1,064.2** | **0.91×** | **0.80×** |

Two readings, and the second is the one nobody expected.

**The native dev path now beats bun on the suite as a whole and on two of four
kernels outright** — 0.91×, from 2.41× before the wave. What is left is K3
(1.56×, now 65% arithmetic and 35% the out-of-line `buri_rt_list_get` call and
its `memmove`) and K2 (1.19×, whose 566 k small allocations at ~20 ns are
~11–17 ms and whose remainder is the recursion). Both are bounded by things the
wave deliberately did not touch: the out-of-line `list.get` and the allocator's
lack of size classes.

**And LLVM at `-O2` buys 1.24× over the optimized dev path — not the 3–6× the
release row's *lowering* cost would suggest.** Per kernel it is nothing at all on
K1 (`-O2` is 1.6% *slower*, and the copy-and-patch investigation measured the
same thing independently: this loop is latency-bound on a 64-bit signed divide,
so there is nothing for an optimizer to shorten), 1.24–1.31× on K2 and K3, and
1.86× on K4. On ordinary straight-line library code it is less again: the
104k-line bulk library's entry point runs in 14.2 ms at `-O2` against 16.2 ms
dev, a 1.15× that is mostly the 2 ms process floor. **The dev/release runtime
band on this workload is 1.2×** — which is a different thing to reason about
from the several-fold band the release backend's lowering rate implies.

#### 6.9.4 What `--release` costs to build

`buri build --release` selects LLVM (`--features backend-llvm`, LLVM 21.1.2).
The same binary — a `main` over the whole 104k-line bulk library, 368 codegen
units — built both ways, medians of five. The incremental edit here perturbs the
body of a function the entry point already reaches, because a binary's codegen
key is its rendered IR and a *dead* new function never moves it.

| | 12k dev | 12k release | 104k dev | 104k release | release ÷ dev |
|---|---:|---:|---:|---:|---:|
| cold | 417 ms | 2,123 | 2,472 | **19,532** | 5.09× / **7.90×** |
| incremental, one live leaf edit | 256 | 285 | 962 | 1,104 | 1.11× / **1.15×** |
| noop | 147 | 161 | 677 | 688 | 1.09× / **1.02×** |
| artifact bytes | 1,228,720 | 708,448 | 7,887,648 | **2,748,928** | 0.58× / **0.35×** |

**The 7.9× is a cold-build number and it does not survive the action cache.** A
one-line edit costs 15% more at `-O2` than at `opt_level=none`, because per-unit
emit re-optimizes one unit of 368 and reads the rest out of the
content-addressed store; a noop costs 2% more, which is to say nothing, because
what is left is whole-program analysis that both profiles pay identically. The
right way to state the release backend's price is therefore **"7.9× the first
time and 1.15× every time after"**, and §6.6's "15–26× under goal 3" remains the
right statement about its *lowering rate* and the wrong one about a developer's
loop.

**One gap found while taking this column, and it is a real one.**
`buri test --release` cannot run a test suite at all: *"the llvm backend has no
implementation of testing_assert.report"*. That is the third native-test gap on
the record beside `core/testing/context` and `list.sortBy`, and it is why the
release column above is `buri build` over a binary rather than `buri test` over
the same suites the other columns use. Nothing about the runtime comparison
depends on it — the kernels are byte-identical source in both — but the release
profile cannot currently be verified by the test suite it is meant to ship.

#### 6.9.5 Cranelift at `opt_level = "speed"`: refuted, on both halves

§6.5 recorded `opt_level = "speed"` as costing 20% of compile time and left its
runtime gain unmeasured; the note in `backend/cranelift/mod.rs` says the same.
This round measured both halves against the same build behind a temporary
env-var knob, since removed.

**Compile cost**, the 97 `--quick` rows, default against `speed`:

| row | default | `speed` | delta |
|---|---:|---:|---:|
| `many-small-fns/1k` lower+macos-arm64 | 9.01 ms | 17.56 | **+95%** |
| `mixed/1k` lower+macos-arm64 | 18.28 | 22.03 | **+21%** |
| `enum-heavy/1k` lower+macos-arm64 | 44.61 | 51.57 | **+16%** |
| every non-native row | — | — | **0%** |
| median over all 97 rows | — | — | **0.00%** |

At workspace scale it is +14% on a cold 104k build (2,861 → 3,256 ms of compile)
and **+0.6% incremental** (1,175 → 1,181), for the same reason the release
column is cheap incrementally: one unit of 377 is re-emitted.

**Runtime gain**, the four kernels, same protocol as §6.9.3:

| kernel | `none` | `speed` | delta |
|---|---:|---:|---:|
| K1 primes | 239.5 ms | 230.8 | −3.6% |
| K2 n-queens 12 | 314.7 | 301.2 | −4.3% |
| K3 matmul 320² | 273.8 | 274.0 | ±0% |
| K4 pipeline | 140.3 | **187.9** | **+34%** |
| **total** | **968.3** | **993.9** | **+2.6%** |

**The trade is not close, and it is worse than "not worth it": the suite is
slower.** Three kernels move by less than the compile cost of moving them, and
K4 — the one shape the wave just made fast — regresses by a third, because the
egraph mid-end rewrites the fused loop into something its register allocator
likes less. `opt_level = "speed"` costs 16–95% of native lowering and returns
−2.6% of runtime. **Cranelift stays at `opt_level = "none"`, now for a measured
reason on both sides of the trade rather than one.** The knob was removed after
the measurement; `git diff` on `backend/cranelift/mod.rs` is empty.

#### 6.9.6 Where every goal stands, mixed/100k, this build

| Phase | Goal | Measured | Gap | vs §6.6 |
|---|---:|---:|---:|---|
| lex | (10 M shared) | 11.09 M | **MET** | holds |
| lex+parse | 10 M | 6.02 M | 1.66× | holds |
| sema | 1 M | 1.06 M | **MET** | +2% |
| lower+js | 100 k | 282 k | **MET** | +4% |
| lower+macos-arm64 | 100 k | **58.1 k** | **1.72×** | **51–54 k → 58.1 k** |

Native lowering is the only row that moved, and it moved because of item 4: the
shared joiner is worth ~9% on any corpus with derived `Show` in it, and `mixed`
has some. It is the first time that row has been under 1.75× off goal 3.

**And the dev/release configuration question, restated with both halves
measured:**

- **Dev: Cranelift at `opt_level = "none"`, whole-binary link, per-unit emit.**
  LLVM at `-O0` is 2.1–4.9× slower to lower (§6.5); `opt_level = "speed"` costs
  16–95% of lowering and *loses* 2.6% of runtime (§6.9.5); erasure and placement
  changes were refuted (§6.5). The path now runs the four kernels 0.91× of bun
  and 1.24× of its own release build.
- **Release: LLVM at `-O2`.** 7.9× a cold dev build and **1.15× an incremental
  one**, for 1.24× the runtime and 0.35× the artifact size. It cannot currently
  run a test suite (§6.9.4).

What remains open, in the order it matters: `buri_rt_list_get`, which is the
whole of K3's remaining 1.56× and is an out-of-line call that bounds-checks and
`memmove`s one element into an `Option` payload — open-coding it the way
`list.map` already open-codes its loop is the next 2× on that shape; the
producer half of fusion (`range` is still materialized); derived `Show`, which
needs the design decision in §6.9.2 rather than more tuning; the LLVM backend's
`testing_assert.report`; realistic native lowering's last 1.72×; and lex+parse's
last 1.66×.

#### 6.9.7 Verification

| Check | Result |
|---|---|
| `cargo test -p buri` | **736 passed, 0 failed** |
| `… --features backend-llvm` | **810 passed, 0 failed** |
| `--quick`, 97 rows | median **0.00%** against the same build; `enum-heavy/1k` at 44.61 ms against the wave's 43.85 |
| `--set=native`, 13 rows | `enum-heavy/10k` **444.1 ms** (485.1 before the wave), `wide-match/10k` **316.4 k lines/s**, `mixed/10k` 60.9 k — all within the ±10% run-to-run band |
| every kernel's own assertion | K1 283,146 · K2 14,200 · K3 > 0 · K4 4,114,354,282,000, identical under dev, release, `speed` and bun |
| temporary knob | removed; `git diff cli/src/compiler/backend/cranelift/mod.rs` is empty |
| tree footprint | `git status --porcelain` is 73 lines at the end as at the start; no tracked file was written by this round outside this document and `blog/` |

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

**And when neither profiler is installed**, which was the case when §6.7 was
taken, the substitute is a controlled sweep: the suite's whole parameter space
is a command-line flag, so a suspected superlinear term can be tested by
holding everything constant and moving the one axis it is suspected in.
`--param lines_per_module=2500` at a fixed `--scale` changes the codegen unit
count and nothing else that matters, and the row moving back to its old rate is
a stronger statement than a flame graph: a profile says where the time is, and
an experiment like that says what the time is a function of. What a profile cannot
give here is cycles-per-line; the substitute discipline is to re-run the suite
after every change and let §6's table, not the profile, say whether the change
was real.

**Two sweeps beat one**, which is §6.8's contribution to this section. The first
sweep there moved enum width and produced a curve that fell and then flattened —
suggestive, and not an answer. The second held the width and moved the derive
load, and the answer was a step function between one derived trait and the next.
An axis that a single sweep leaves ambiguous is often two axes, and the second
sweep is cheap: the whole parameter space is a command-line flag, and both of
those sweeps together were under ten minutes at a scale small enough to be
quick and large enough to be real.

The two are complements rather than alternatives, and §6.7's fix is the case
that shows it. The sweep named the axis — the unit count — and the two suspects
it made obvious were two thirds of the cost; the third was a
strongly-connected-components pass inside a constructor, which nothing about the
axis suggested and a profile pointed straight at. **macOS ships one**:
`sample <pid> <seconds> 1 -file out.sample` needs nothing installed, and its
"sort by top of stack" section is enough to read a self-time ranking. It is a
poor substitute for `samply` — no inverted call tree worth the name, and
Rust's mangled symbols come out raw — and it is much better than nothing.

