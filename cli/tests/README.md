# Testing the toolchain

A compiler that exits 0 has proved nothing. These suites are built so a wrong
*answer* fails, not merely a program that fails to run.

## The tree

A **directory holding a `main.rs` is a test binary**. Everything else is a
**corpus** that one or more of them reads. Cargo discovers `tests/<name>/main.rs`
on its own, so a domain is a directory and needs no entry in `Cargo.toml`.

```
cli/tests/
  harness/              shared machinery, not a binary: the CLI runner and its
                        per-invocation hang cap, the scratch repository, the
                        bless-or-compare loop, and the seeded single-token
                        mutator `recovery.rs` draws from

  language/  main.rs    WHAT THE LANGUAGE DOES, on the reference backend
    conformance.rs        the conformance repository, the reject corpus
    standard_library.rs   core/* against itself
    corpus.rs             every source in the repository, as a corpus
    golden_javascript.rs  what the JavaScript backend emits
  build/     main.rs    THE BUILD SYSTEM, driven as a user drives it
    repositories.rs       one repository per build-system rule
    example.rs            the worked monorepo
    incrementality.rs     what the cache may and may not do
    hermeticity.rs        spawn determinism, concurrency, reproducibility
    watch.rs              the input set, and what an edit re-runs
  native/    main.rs    THE NATIVE BACKENDS, and the runtime they link
    link.rs               bytes in, an executable out
    runtime.rs            the buri_rt_* C ABI, driven from C
    driver.c              …the C it is driven from
    float_parity.rs       3.8 million doubles, native `show` against JS
    conformance.rs        gated on `backend-stencil`
    llvm.rs               gated on `backend-llvm`
    stencil.rs            gated on `backend-stencil`: the copy-and-patch
                          backend, its leak parity, and its cross emission
    agreement.rs          gated on either: VALUE-MODEL.md §12, both backends
    e2e.rs                gated on either: WHOLE PROGRAMS, real processes, real
                          sockets, real signals — the top of the trust ordering
    shared.rs             what more than one backend suite needs and none owns
  docs/      main.rs    THE DOCUMENTATION, held to the bar the code is
    documents.rs          what the documents are: fences, links, staleness
    examples.rs           what the documents show: every example compiles
  vectors/   main.rs    GROUND TRUTH FROM OUTSIDE, replayed offline
    lean.rs               the Lean model's exhaustiveness verdicts
    proto.rs              protobuf's own conformance exchanges
  formatting.rs         THE FORMATTER — a domain of one, its own binary
  adversarial.rs        HOSTILE INPUT — deliberately its own process
  failing.rs            A FAILING RUN — the report a user reads, pinned
  fuzz.rs               INPUT NOBODY CHOSE — generated and mutated, against
                        properties, with its findings recorded as it goes
  recovery.rs           ONE TOKEN WRONG — what the toolchain says about a
                        mistake, as invariants over the whole corpus
  checking.rs           THE SAME MISTAKES, PINNED — one broken source and the
                        page the front end prints for it, case by case
  linting.rs            AND WHAT THE RULES STILL SAY — a lint fixture with one
                        token wrong, through a one-package repository
  ci.rs                 THE BUILD THIS TOOLCHAIN WAS BUILT BY — what
                        `.github/workflows/ci.yml` promises, and the liveness
                        gates that say the toolchain did not degrade

  conformance/          a Buri repository: `test/` blocks on language semantics
  reject/               programs that must not compile, with their diagnostics
  crash/                programs that compile, then abort, saying why
  example/              the worked monorepo, and the largest Buri here to read
  repositories/         whole repositories, one per build-system rule
    concurrency/          …and one per concurrency-and-servers claim: actors,
                          scopes, arenas, the sockets double, imports, a
                          print's Result
  golden_javascript/    one construct per case, with the code it emits
  formatting/           an `input.buri` and the one `expected.buri` allowed
    generated/          the same, a thousand of them, written by the mutator
  checking/             clean/ and cascades/: a broken source and its page
  linting/              a broken lint fixture and the findings it still draws
  proto/                vendored schemas, a testee, and the recorded exchanges
  failing/              one directory per failure shape, with its report
  fuzz/                 every finding a search has made, minimised, replayed
  recovery/             one hand-written case per list context, with the exact
                        message, span and edit it must produce
  message-audit/        run.sh: the diagnostics put to a model, one question
```

**Thirteen binaries**, so a full run links thirteen times. Corpora sit at the
top level because suites share them: `language::conformance` reads
`conformance/` on the JavaScript backend while `native::conformance` and
`native::stencil` read it on the copy-and-patch one, and four suites read
`crash/`. That split is written into the corpus — each
`conformance/lib/*/BUILD.buri` declares `test { platforms: [JS] }`, because
`buri test` runs a suite natively by default and the reference run has to stay
the reference one. A divergence then means one backend is wrong, rather than two
sets of assertions disagreeing.

## The suites

| Binary | What it proves |
|---|---|
| Unit tests (`cli/src/**`, `#[cfg(test)]`) | The lexer, parser, textproto reader, type unifier, JS printer, minifier, SHA-256, and SCC finder do what they claim in isolation. |
| `language` | That a program means what SPEC says. The conformance repository through the real `buri test`, the reject corpus with its diagnostics recorded exactly, `core/*` typechecking against itself, every source in the repository parsing and formatting to a fixed point, and what the JavaScript backend compiles each construct to. |
| `build` | What the build system does. One repository per rule with a manifest of what the CLI does in it, the worked monorepo, what the cache may and may not do read off `--explain`, that an action's spawn is deterministic and a perturbed environment changes neither bytes nor verdicts, and what `buri watch` declares and re-runs. |
| `native` | That the native backends agree with the reference one, and that the runtime under them holds. Bytes in and an executable out, the `buri_rt_*` C ABI driven from C, 3.8 million doubles of float rendering, whole programs through the copy-and-patch backend and LLVM, and VALUE-MODEL.md §12 row by row under both. `e2e` sits at the top of the trust ordering below. |
| `docs` | That every fence is scannable and tagged, every link resolves, the assembled `SPEC.md` is not stale, and every example in every topic — and in the root `README.md` — compiles. |
| `vectors` | That the Lean formalisation and protobuf's own conformance runner still agree with this toolchain. It replays checked-in vectors, so the suite needs neither tool installed. |
| `formatting` | A directory per decision the formatter makes. Plus: every output is a fixed point, keeps its comments and tokens, and fits the margin. |
| `adversarial` | That no input panics the toolchain. Malformed sources, build files, schemas, flags and language-server messages, through the binary, asserting on *how* it stops rather than on what it says. |
| `fuzz` | That the properties every other suite states over a corpus also hold over input nobody chose. No mutation of a checked-in source panics the toolchain; the formatter is a fixed point that keeps its tokens and comments on shapes nobody wrote; the benchmark generator emits programs that compile at points no profile names; the same input twice says the same thing; and a generated program prints the answer the generator computed — under every backend, and with unreachable code inserted into it. Bounded and seeded in CI; `BURI_FUZZ_SECONDS` soaks. |
| `recovery` | That one mistake reads as one mistake. Every compiling source in the repository, with a token deleted, inserted or exchanged, held to six invariants: one diagnostic per mistake, its caret at the mistake, its `fix` naming the token, no type error invented downstream, and the file still formatting. Plus a hand-written case per list context pinning the exact message, span and edit. Each invariant is held per mutation shape. Where the mutated text has a second reading the grammar accepts, it is held against a ceiling rather than zero, and `ceiling()` states which rows those are. A ceiling is a **percentage of the row's population**, read off a `BURI_RECOVERY_CAP=0` run, so a source landing in the repository cannot tip a row whose per-case behaviour did not change. |
| `checking` | The same mistakes, pinned rather than counted: seven hundred mutated sources with every error the front end reports about each one recorded beside it. A case whose errors are exactly the parser's lives under `clean/`; one the mistake led the checker into lives under `cascades/`. So a recovery that stops a cascade shows up as a file changing sides. |
| `linting` | What `buri lint` still finds in a file that did not parse whole: each lint fixture's source with one token wrong, through a repository of one package per case, with the whole report recorded. Two rate ceilings over the population — the mistake invents no finding, and a finding whose evidence survived still fires — plus a parity: every case compares the set `buri lint` prints against the set the language server publishes, because two halves that go quiet together keep any rate they like. |
| `failing` | That a failing `buri test` fails *well*. It pins the report a user reads — line, expected, got, counts, exit code — byte for byte across every value shape, abort, title edge case and multi-module ordering. |

Everything but the unit tests drives the real `buri` binary, because that is what
a user runs.

## The trust ordering

Which suite to believe when two disagree, and the rules that keep the ordering
from being a preference nobody enforces.

### The tiers, most trusted first

| | Tier | What it runs | Where it lives |
|---|---|---|---|
| 1 | **End to end** | A whole program, built by a real backend, in a process of its own, talking over a real socket and receiving a real signal. | `native/e2e.rs`, and the server rows in `native/stencil.rs` and `native/llvm.rs` |
| 2 | **Repository** | A repository on disk, one `buri` command, and what it printed — the CLI a person types, over a package graph, with a linked test binary underneath. | `repositories/*`, `failing/`, driven by `build::repositories` and `failing.rs` |
| 3 | **Conformance** | The same corpus of `test` blocks under every backend, so a divergence is a failure in exactly one of them. | `conformance/`, read by `language::conformance` and `native::conformance` |
| 4 | **Agreement** | One source through two backends, compared byte for byte. | `native/agreement.rs` |
| 5 | **Runtime crate** | `cli/runtime`'s own `#[test]`s: the acceptor, TLS, WebSocket framing, the mailbox, the arena — both ends of a wire, inside one process. | `cli/runtime/*.rs`, reached by `native::runtime` |
| 6 | **Compiler unit** | A function, a hand-built `Program`, a table. | `cli/src/**` `#[cfg(test)]` |

The higher a tier, the fewer of its assumptions are the test's own — and the
less precisely one failure names what broke. That is the trade, and it is why no
lower tier is demoted. A red row in tier 5 says which function; a red row in
tier 1 says which program.

The ordering came out of an incident. A rename stopped `failing/task_order/`'s
fixture compiling, a later commit blessed its golden down to
`0 passed, 0 failed, 0 skipped, 1 failed to compile`, and everything under it
stayed green for two waves. **A lower tier staying green says nothing about
whether the thing a user runs still works.**

### The rules

**1. Every feature's happy path *and* its signature failure get a test at the
highest tier that can reach them.** Not a rate, not a sample: the two named
cases. A green happy path says a thing worked on the day somebody wrote it; the
failure beside it says the mechanism is still connected. So `e2e::a_tls_port_…`
proves a TLS port answers TLS *and*, on the same listener a moment earlier, has
nothing to say to a plaintext client. Every case under
`repositories/concurrency/` ends with the edit that makes its subject fail, and
records the report.

**2. A golden can never collapse into the absence of one, silently.**
`harness::no_golden_has_collapsed` runs at the end of every `run_corpus` and at
the end of `failing.rs`'s `recorded_failure_reports`, over the bytes the run
just wrote. It is set equality in **both** directions against
`harness::A_RUN_THAT_ASSERTED_NOTHING`. So a case that starts recording
`failed to compile` or `0 passed, 0 failed` fails, and a case that is
*supposed* to record one and has stopped fails too. The corpus runner calls it
rather than it being a `#[test]` of its own: under `BURI_BLESS=1` a separate
test races the blesser and reports the golden it is in the middle of fixing.

Blessing may rewrite what a report *says*. This is the one thing it may not do.

**3. Every end-to-end test binds loopback, and every wait has a deadline.** A
server thread once sat in `accept()` with no deadline while the test thread
called `join()`, on a host where `localhost` resolves to `::1` first, and it
took a CI job with it. So:

* the program binds `port: 0` and prints the port; the client dials
  `127.0.0.1`. A test that picks a port and hopes races the pick against the
  bind;
* every connect, read and write carries `shared::SERVER_DEADLINE`;
* **read a reply until it is whole, never to a byte count.** A loop that stops
  at "some bytes arrived" asserts about whatever the kernel coalesced into one
  segment. `e2e::Until` lets the caller say what a complete answer *is*: the
  peer closed, or one whole TLS record. `e2e::read_loop_tests` is that rule with
  a peer of its own that answers in two writes, so the loaded-runner split
  happens every time instead of on CI only;
* every wait on a child goes through `shared::waited`, which polls and **kills
  what it could not stop** — `Child::wait` has no deadline;
* join every thread, and read the client's answer *before* joining the server,
  so a client that failed reports as the client failing;
* two things bound a `#[test]` here: the harness's per-invocation hang cap
  (`harness/hang.rs`) for anything it spawns through the CLI, and each CI job's
  `timeout-minutes` outside that.

A broken server is a failing test with a sentence, never a job CI has to kill.

### The exceptions, and why each is one

Three claims cannot reach the tier they belong at, all because of
`language::corpus::dependencies_stay_behind_the_bar`: the workspace may not grow
a dependency for a test, `[dev-dependencies]` included.

**A test cannot use a TLS or WebSocket library.** The only `rustls` and the only
`tungstenite` here sit inside the runtime archive, so the protocol-level claims
— handshake, multiplexing, masking, fragmentation, the close handshake — live at
tier 5 in `cli/runtime/net.rs`, where both ends of the wire are reachable. Tier
1 asserts the half only it can: that a *Buri program's* `Server` opened the port
that answered. Its clients are hand-written. `e2e::client_hello` is the smallest
TLS 1.2 `ClientHello` a `rustls` server will answer, at 1.2 precisely because
1.3 encrypts the ALPN answer. `shared::Talking` is one masked text frame at a
time.

**A repository fixture cannot run a server.** `repositories/` reaches a native
backend through `buri test`, and a test source may not import `core/host` (SPEC
4.1.1). So a repository case asserts the *graph* refusal —
`build-files/server_on_a_page` — and anything that runs a listener sits at tier
1, in a process of its own.

**`networking-not-available` has no whole-process row.**
`runtime_native::net()` reads a file `cli/build.rs` writes beside the archive
and `include_str!` bakes into the binary, so *running* that refusal would mean
building `buri` a second time with `BURI_RUNTIME_NET=0`.
`e2e::the_refusal_a_toolchain_without_networking_names_what_a_real_server_reached`
stands in its place: it drives the real front end over a real `server.bind`-and-
`run` source and asks the refusal's own seam what it would say, recording the
eight intrinsic keys such a program actually reaches.

## Running them

```
cargo test -p buri                                    # everything
cargo test -p buri --test language                    # one domain
cargo test -p buri --test language conformance::      # one suite in it
cargo test -p buri --test native -- --skip float_parity
cargo test -p buri --features backend-llvm --test native
BURI_RECOVERY_CAP=0 cargo test -p buri --test recovery   # every case, not a stride
```

A module is a name prefix, so `--test language conformance::` selects exactly
what `--test conformance` used to, and `--skip` takes a module out the same way.

The first line is the whole suite, and the `release` job runs exactly that. The
`test` legs run **the same set with its binaries overlapped**, in four lines of
inline shell in `.github/workflows/ci.yml`:

```
cargo test -p buri --no-run --message-format=json-render-diagnostics \
  | jq -r 'select(.profile.test == true) | .executable | select(. != null)' > units.txt
xargs -P "$(( $(getconf _NPROCESSORS_ONLN) * 2 ))" -n 1 \
  sh -c 'exec "$0" --test-threads=2' < units.txt
```

`cargo test` starts the test binaries one after another, and these suites are
latency-bound rather than core-bound, so most of the machine sits idle. On run
33981313436's arm64 leg the step took 424 s, of which **145 s was compiling and
280 s was running fifteen binaries in a queue**. Started together on a ten-core
mac the same fifteen take **83 s against 165 s**, for about 400 CPU-seconds
either way.

Both numbers in that snippet are measured. `--test-threads=2`, because a core
with one runnable thread idles through the `cc` and child-process stalls the
second hides. **Twice** `nproc` of workers, because `native` is the longest unit
by a factor of two and cargo emits it fourteenth of fifteen; at `nproc` workers
it starts last and holds the run open alone. Sorting the list is the other fix
and we do not take it: neither binary size nor test count predicts runtime here,
so any order worth having would be a hand-written list of domain names, and
`ci.rs::the_suite_is_asked_for_as_a_whole` forbids one.

**Where a failure's evidence lands.** Every suite works on a copy under
`CARGO_TARGET_TMPDIR`. Nothing writes into a checked-in tree, so the suites hold
no lock, run in parallel, and two `cargo test` runs in two shells do not
collide. `BURI_KEEP=1` leaves the scratch directories behind, and a panicking
test leaves its own regardless: a failing test's evidence is the directory it
failed in.

A run also sweeps that directory once, before it makes its first tree
(`harness/sweep.rs`). Most scratch removes itself when its `Scratch` drops. The
native suites' per-process trees — `native-stencil-<pid>` and its siblings,
about 180 MB a run — are named for the process so two overlapping runs cannot
share one, and nothing deletes them; fourteen gigabytes of them filled a disk
twice. The sweep takes only what has not been written to for **two hours**,
which no live run can manage, and it does not run at all under `BURI_KEEP`.

### Reproducing a Linux CI leg on a mac

A `test` job is a container with the toolchain in it, then the same
`cargo test -p buri`. The container needs `CC=clang` and the musl `rust-std`:
`cli/build.rs` degrades silently without either, and `ci.rs`'s liveness gates
say so before the suite spends ten minutes proving nothing.

```
docker run --rm -it -v "$PWD":/w -w /w rust:latest bash -c '
  apt-get update &&
  apt-get install -y --no-install-recommends clang lld mold llvm binutils musl-dev musl-tools &&
  rustup target add "$(uname -m)-unknown-linux-musl" &&
  CC=clang BURI_CI=1 cargo test -p buri'
```

`--platform linux/amd64` on the same line runs the x86-64 leg under emulation,
slowly enough to be an overnight answer rather than an edit loop. `BURI_CI=1`
turns a guard that fires into a failure rather than a quiet pass, which is the
whole reason to run the leg.

### Skips: none on CI, and each one on a host has a name

A test can fail to run here in three ways, and each has an answer.

**`#[ignore]`.** There are **none** in the whole repository, and
`.github/known-skips.txt` is the empty list that says so.
`cli/tests/ci.rs::the_only_ignored_tests_are_the_ones_named_here` walks the tree
and fails if the set it finds is not exactly that file's. In order of
preference: fix it; `#[cfg]` it out on the host that genuinely cannot answer it,
so it is absent rather than reported as not run; or delete it, if nobody wants
the behaviour it asserts. A row in that file is a named defect rather than a
permission.

**`if !supported() { return; }`.** The native suites open with one, load-bearing
on a host with no C compiler or no stencil library for its triple. On CI it is
the one shape of green this repository refuses, because every runner installs
the tools and asserts the stencil libraries and the runtime archive are real
bytes before the suite starts. So `BURI_CI=1` — set in the workflow's `env:`
block, and therefore in every job — makes `harness/ci.rs::skipped` PANIC instead
of returning. Set it locally to see what a runner sees.

**Deferrals.** `repositories::language_server_speed` and
`repositories::language_server_open_cost` assert milliseconds rather than work,
so they return early unless `BURI_PERF` is set, and they mean nothing outside
`--release`:

```
BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_
```

Both hold every editor request to 50 ms, and neither fails on a single reading:
a run over the bar measures its whole session again, up to three times, and
holds each request to the fastest time it was seen in (`best_of`). The
measurement repeats, not the assertion. CI runs them on its arm64 runner
(`.github/workflows/ci.yml`, `language-server-budget`), where
`BURI_PERF_BUDGET_SCALE` widens the bar for a slower machine. They name that job
through `ci::deferred_to`, and
`cli/tests/ci.rs::every_deferral_names_a_job_that_still_asks_for_it` holds the
name to a job that exists — a deferral whose job has been renamed is a plain
skip.

### The runtime crate's tests are run by a test

`cargo test -p buri` cannot reach the `cli/runtime` cargo package, so
`native::runtime::the_runtime_crate_answers_its_own_tests` runs its ninety-seven
assertions instead. It shells a nested `cargo test` against the package
`cli/build.rs` assembles in `$OUT_DIR`, with the same features the archive
beside this binary was built with. That cold-compiles tokio and rustls the first
time — about ten seconds here, a minute on a cold runner — into a target
directory under `CARGO_TARGET_TMPDIR`, and once per checkout after that.

### The five-minute budget

The whole verification bar runs in **under five minutes**. That is a policy: a
change that pushes it over owes either an optimization that brings it back or a
written justification beside the change. **Coverage never pays for it** — the
way back under the line is faster mechanics, never running less — and **the
number is measured, not asserted**, so a change that touches this suite's cost
reports the bar's wall time against the budget in its verification section.

The bar is this sequence:

```
cargo test -p buri
cargo test -p buri --features backend-llvm --lib compiler::backend::llvm::
cargo test -p buri --features backend-llvm --test native -- llvm:: agreement:: e2e::
cargo test -p buri --features backend-llvm --test fuzz
cargo bench  -p buri --bench compiler --profile validate -- --validate
cargo clippy -p buri --all-targets
cargo clippy -p buri --all-targets --features backend-llvm
```

`LLVM_SYS_211_PREFIX` has to be set for the three feature lines and for the
second clippy. On a quiet ten-core M-series mac it is 122 s warm and 176 s after
a `cli/src` edit, which is the column that matters because it is the loop.

**Why the feature leg is three lines rather than one.** A plain
`cargo test -p buri --features backend-llvm` runs 917 tests, and 843 of them are
what the first line just ran, with the same code, to the same answer. The delta
is 74 tests by name — fifteen `backend::llvm` unit tests and the 59 in
`native::llvm` — plus three suites whose existing tests do more work under the
feature: `native::agreement` and the whole `fuzz` binary each keep a `NATIVES`
table that gains an `llvm` row, and `native::e2e` builds its whole programs with
whichever native backend the toolchain has. Running those five selections
instead of the other 788 is **dedup, not less coverage**.

It is only dedup while the delta stays where we say it is, so that is a test.
`language::corpus::the_llvm_feature_is_confined_to_the_files_the_bar_names`
reads every `.rs` file under `cli/src` and `cli/tests` and fails if a
`cfg(feature = "backend-llvm")` appears outside those files. One hole it cannot
cover: `backend::select` answers with LLVM for a *native release* build, so a
test driving one through the CLI would differ under the feature with no `cfg` of
its own. Every `--release` in the suite today builds a `platform: JS` output,
and `native_ready` rejects `Js` before anything reaches `select`. A change that
adds a native `--release` test owes this paragraph a second look —
`compiler::backend::a_release_refusal_names_the_profile_rather_than_the_platform`
takes it today, asking `build::actions::native_gap` for the host's own target
under `Profile::Release` and asserting both answers.

**CI is not this.** CI runs everything under both feature sets, on both hosts.
The sequence above is the local edit loop. One line of it *is* CI's: the
validation gate runs under `--profile validate` there too, which is what the
root `Cargo.toml` declares that profile for.

## Why each shape exists

**The conformance repository** is ordinary Buri. Its packages are libraries
whose `test/` directories hold `test "..." { ... }` declarations, so each case
exercises the whole pipeline — parse, check, monomorphize, emit, minify, run —
and asserts on a value rather than on an exit code. `lib/canary` keeps the suite
honest: `conformance_suite_can_fail` rewrites a constant in it and asserts the
runner notices.

**Rejection and abort corpora** cover what assertions cannot. A program that
must not compile has no runtime to assert in, and nothing can catch an abort —
the language has no `catch` — so both get a harness that compiles or runs the
program and checks the diagnostic. Each file carries its expectation on its
first line:

```buri
// EXPECT: may not be discarded      (tests/reject)
// CRASH: division by zero           (tests/crash)
```

A reject case is a directory, carrying a second and much stricter expectation:

```
cli/tests/reject/non_exhaustive_match/
  main.buri       the program
  expected.txt    the diagnostics, exactly as a terminal shows them
  expected.json   the same, as `--error-format=json` emits them
```

The `// EXPECT:` line says what the case is *about*. The two recorded files pin
what a user actually reads: span, carets, notes, the order of several
diagnostics, every word of the prose. A reworded message changes the product, so
it should turn up as a diff somebody looks at rather than pass because a
substring survived. After a deliberate change:

```
BURI_BLESS=1 cargo test -p buri --test language conformance::rejected_programs
```

The JSON file also enforces the four-part contract: **every diagnostic must
carry a `fix`**, and the harness fails the case if one does not.

**The formatting corpus** is a directory per decision the formatter makes,
holding an `input.buri` somebody might have typed and the one `expected.buri` it
is allowed to produce. There is no third file, because there is nothing to
configure. `formatting.rs` also holds every output to being a fixed point, to
keeping the comments and tokens it was given, and to fitting the margin — the
last except in the `width_*` cases, named for the atoms that cannot break. A
`NOTES.md` beside a case marks a shape that is pinned rather than endorsed.

```
BURI_BLESS=1 cargo test -p buri --test formatting
```

A case named `textproto_*` is a build file rather than source. `buri format` has
two printers and this corpus pins both, and
`every_checked_in_build_file_is_formatted` holds the repository's own two
hundred `BUILD.buri` and `REPO.buri` files to what the second one prints.

A case named `recovery_*` is source with a **syntax error** in it, and it pins
what the formatter does about that: the declaration the parser could not read
comes back byte for byte, and everything around it gets laid out. Every other
case's input has to parse, and the harness says so rather than quietly
formatting a broken file. The four claims hold for these too. The output is a
fixed point. It keeps every comment and token. It carries the same number of
syntax errors as its input, with every region byte for byte what was written.
And every line the formatter *laid out* fits the margin.

This corpus sits outside the repository-wide walkers on purpose: an
`input.buri` is misformatted deliberately.

**The generated corpora** are the same bargain at three orders of magnitude, and
a program writes them. `formatting/generated/` holds a thousand `recovery_*`
pairs, `checking/` seven hundred sources with the page the front end prints for
each, and `linting/` six hundred lint fixtures with the findings they still
draw. `harness/pinned.rs` samples all three from `harness/mutation.rs`'s
population, one case per **coverage cell**: the mutation kind, the delimiter
open at the site, the declaration around it, what opened that delimiter, and the
tokens either side. A cell keeps at most two, and the smallest seed that
exhibits it wins.

Every one of them regenerates, and each suite has the test that says so: run the
sampler again from the same seed over the same sources and it chooses exactly
the checked-in cases and writes exactly their bytes. Blessing is therefore
idempotent, and one command regenerates all three:

```
BURI_BLESS=1 cargo test -p buri --test formatting --test checking --test linting
```

One constant per suite holds the count — `GENERATED_TOTAL` and `TOTAL` — so
scaling the corpus is a number and a bless.

**The fuzz corpus** is the one corpus nobody wrote. A search found every case in
it, minimised it, and wrote it down so the finding cannot be lost:

```
cli/tests/fuzz/generated_derive_with_an_empty_trait_list/
  CASE.textproto      doc, which property failed, and whether it is still open
  input.params        the minimised input — `input.buri` for the five
                      properties whose input is source
```

`status` is what lets a fuzzer live in a suite that has to stay green. `FIXED`
is the ordinary regression: the property must hold. `OPEN` is the reverse — the
property must **still** fail — so the day somebody fixes it the suite fails and
says to move the case to `FIXED`. The searches skip a finding whose signature
the corpus already holds, so an open bug does not hide every bug behind it.

```
BURI_FUZZ_SECONDS=600 cargo test -p buri --test fuzz     # soak
BURI_FUZZ_RECORD=1 …                                     # write findings down
```

Recording is opt-in because nothing else here writes into a checked-in tree.

**The repository corpus** exists because the two corpora above cannot hold a
build-system test. A reject case is synthesised as a single-package binary with
no dependencies, so nothing in it can express `missing-dep`, `dep-cycle`,
`visibility-violation`, or a tag conflict. A case there is a repository instead:

```
cli/tests/repositories/build-files/missing_dep/
  CASE.textproto      the manifest
  repo/               the repository, copied into a scratch tree and run in
  expected/lint.txt   what the CLI printed, recorded
```

The manifest is textproto, read by the toolchain's own parser. It deliberately
does *not* end in `.buri`: only the tree under `repo/` is a Buri repository.

```
doc:  "one line saying what the case is about"
run  { args: ["lint", "//cmd/app"]  exit: 1  golden: "lint.txt" }
run  { args: ["build"]  exit: 0  cwd: "lib/money" }
edit { file: "cmd/app/BUILD.buri"  replace: "..."  with: "..." }
file { path: "cmd/f/main.buri"  golden: "formatted.buri" }
path { path: ".buri/out"  exists: false }
path { path: "out"  symlink: ".buri/out/js" }
```

`cwd` runs the command from a directory inside the repository, which is the only
way to ask what a command with no target operates on. `path` covers the commands
whose contract is about what they leave on disk rather than what they print:
`clean --outputs` and the `out/` symlink say almost nothing, and an exit code
cannot tell a cache that survived from one that was deleted and rebuilt. Exactly
one expectation per `path` step, and like `exit` it is never inferred.

`exit` is hand-written and required. Only prose gets blessed, so blessing can
rewrite what a diagnostic *says* and can never quietly turn a rejection into an
acceptance — a flipped exit code fails a `BURI_BLESS=1` run too.

Steps run in order against one scratch copy, so a case shows a rule firing and
then shows that the fix the diagnostic printed actually works. **Every case that
must stay clean ends with the edit that makes it fire.** `visibility_skips` is
the shape to copy: a private library reached by its own test suite and its
co-located binary, clean, and then one edge from outside.

Goldens stay path-stable without scrubbing, because every file enters the source
map under a repository-relative name. So `--> cmd/app/main.buri:9:6` in a
recorded file is also a path you can open.

A case about *the platform this toolchain is not* cannot write the platform
down, because `linux` names a target a mac refuses and names the host on a Linux
runner. Such a case writes a placeholder instead:

```
run { args: ["build", "//cmd/native", "--output={{CROSS_PLATFORM}}/{{CROSS_ARCH}}"] exit: 1 }
{ platform: {{CROSS_PLATFORM_PROTO}}, arch: {{CROSS_ARCH_PROTO}} },   # in a BUILD.buri fixture
error: the {{CROSS_PLATFORM}} backend is not implemented              # in a golden
```

The harness fills these in from a table keyed on the host — `linux/x86_64` on a
mac, `macos/x86_64` on Linux — in the fixtures it copies into the scratch tree,
in the manifest's own strings, and in reverse on the way back out. So blessing
on either machine writes the same file. The two hosts' spellings are the same
width on purpose: a caret run is as wide as the line it underlines. The harness
substitutes only the facts a case actually names, so every other case's goldens
come back byte for byte.

**The incrementality suite** reads `buri build --explain`, which prints one line
per action with its key and whether the cache served it. It compares keys
between two states of one tree and never records them, because a key includes
the toolchain version and a recorded one would move on every release.

**The golden transcript** catches a backend that produces a *different* answer
rather than no answer, on the one path no assertion inside a program can reach:
what a whole program prints on its way out. It is one line, so it lives as a
literal in the test rather than in a file — a change to what `//cmd/web` prints
should be read in the diff, not blessed.

**The emitted-JavaScript corpus** is the only suite that looks at the output
rather than the answer. Every other suite here is blind to an optimisation by
construction: removing an allocation, a call frame or a redundant test changes
no value anywhere. Each case is one small program exercising one construct:

```
cli/tests/golden_javascript/enum_match/
  main.buri       the program
  expected.mjs    the generated code, with the runtime removed
  expected.out    what it prints
cli/tests/golden_javascript/sizes.txt   release artifact sizes, whole corpus, one file
```

The runtime comes out because it is the same thousand lines in every case. What
is left is exactly what the backend produced, with debug names, so the diff
reads well. `sizes.txt` is one file rather than one number per case, so a
change's size effect shows up as a single diff. Each case must be smaller in
release than in debug, and each runs in both build modes and must print the same
bytes.

**`expected.out` is recorded once and never re-recorded.** `expected.mjs`
records *how* a program compiles and is meant to move with every pass.
`expected.out` claims what it *computes*, and no pass may move it. So
`BURI_BLESS=1` writes it when it is missing and refuses to overwrite it
afterwards; to change one deliberately, delete it and re-record. A
parallel-move optimisation in the tail-call rebinding once read a parameter
after overwriting it, turning a sum of `5050` into `4950`, and this corpus was
the only thing that saw it.

```
BURI_BLESS=1 cargo test -p buri --test language golden_javascript::
```

Blessing without reading the diff is the one way this suite proves nothing.

## Properties pinned outside the corpora

`language/conformance.rs` also holds the checks about the toolchain rather than
the language:

- **Tail calls run in constant stack on V8.** Ten million bounces through a
  self-recursive function and a mutually recursive pair, on an engine with no
  native proper tail calls.
- **`--release` and `--debug` agree.** Mangling, dead code elimination, constant
  folding and runtime tree-shaking may not change what a program computes or
  prints. The whole conformance suite runs both ways and has to pass the same
  number of assertions, and the monorepo's binary has to print the same bytes.
  The release artifact also has to be *smaller*.
- **Builds are reproducible.** Copy the worked monorepo to two directories,
  build it in each, and the output is byte-identical.
- **The cache cannot serve a stale answer.** Build, edit, rebuild, and check the
  program's *behaviour* changed. Then revert and check the original entry comes
  back.
- **Exit codes distinguish bad code from a bad invocation**, which is the
  distinction CLI.md draws between 1 and 2.
- **`format --check` reports without rewriting**, and formatting is a fixed
  point.

`native/agreement.rs` is the one such check outside `language/conformance.rs`,
and it is about the *pair* of backends. It compiles a single `.buri` source
through `actions::prepare` and `backend::select` twice — JavaScript under `bun`,
native through the copy-and-patch backend or LLVM and `cc` — then compares the
two outputs byte for byte. Every row of `design/native/VALUE-MODEL.md` §12 is a
`#[test]`, so a failure names the row, and
`every_row_of_the_table_names_a_test_that_exists` fails on a row whose test is
missing. A row the native surface cannot reach yet gets a gap test naming the
missing intrinsic *and* an `#[ignore]`d agreement test beside it, so neither can
rot alone. It skips with a printed reason where `native_ready` is false or no
JavaScript engine is on the path. A backend with no seat on this *host* is left
out of the row by name and by reason, from `stencil::unavailable_reason`, so the
rows light up the day the seat lands.
