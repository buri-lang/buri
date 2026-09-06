# Testing the toolchain

A compiler that exits 0 has proved nothing. These suites are built so a wrong
*answer* fails, not merely a program that fails to run.

## The tree

Two kinds of thing live here, and the listing shows which is which. A
**directory holding a `main.rs` is a test binary**. Everything else is a
**corpus** that one or more of them reads. Cargo discovers
`tests/<name>/main.rs` on its own, so a domain is a directory and needs no
entry in `Cargo.toml`.

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
                          arenas, the sockets double, imports, a print's Result
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

**Thirteen binaries**, so a full run links thirteen times: five directories
holding a `main.rs` and eight bare `.rs` files. Suites share corpora, so
corpora sit at the top level rather than inside any one suite's directory.
`language::conformance` reads the `conformance/` repository on the JavaScript
backend while `native::conformance` and `native::stencil` read it on the
copy-and-patch one, and four suites read `crash/`. That first split is written
into the corpus: each `conformance/lib/*/BUILD.buri` declares
`test { platforms: [JS] }`, because `buri test` now runs a suite natively by
default and the reference run has to stay the reference one. Several backends
reading one corpus is the point. A divergence then means one backend is wrong,
rather than two sets of assertions disagreeing.

## The suites

| Binary | What it proves |
|---|---|
| Unit tests (`cli/src/**`, `#[cfg(test)]`) | The lexer, parser, textproto reader, type unifier, JS printer, minifier, SHA-256, and SCC finder do what they claim in isolation. |
| `language` | That a program means what SPEC says. The conformance repository through the real `buri test`, the reject corpus with its diagnostics recorded exactly, `core/*` typechecking against itself, every source in the repository parsing and formatting to a fixed point, and what the JavaScript backend compiles each construct to. |
| `build` | What the build system does. One repository per rule with a manifest of what the CLI does in it, the worked monorepo, what the cache may and may not do read off `--explain`, that an action's spawn is deterministic and a perturbed environment changes neither bytes nor verdicts, and what `buri watch` declares and re-runs. |
| `native` | That the native backends agree with the reference one, and that the runtime under them holds. Bytes in and an executable out, the `buri_rt_*` C ABI driven from C, 3.8 million doubles of float rendering, whole programs through the copy-and-patch backend and LLVM, and VALUE-MODEL.md §12 row by row under both. `e2e` sits at the top of the trust ordering below: whole programs in processes of their own, over real sockets and real signals, built by whichever native backend the toolchain has. |
| `docs` | That every fence is scannable and tagged, every link resolves, the assembled `SPEC.md` is not stale, and every example in every topic — and in the root `README.md` — compiles. |
| `vectors` | That the Lean formalisation and protobuf's own conformance runner still agree with this toolchain. It replays checked-in vectors, so the suite needs neither tool installed. |
| `formatting` | A directory per decision the formatter makes. Plus: every output is a fixed point, keeps its comments and tokens, and fits the margin. |
| `adversarial` | That no input panics the toolchain. Malformed sources, build files, schemas, flags and language-server messages, through the binary, asserting on *how* it stops rather than on what it says. |
| `fuzz` | That the properties every other suite states over a corpus also hold over input nobody chose. No mutation of a checked-in source panics the toolchain; the formatter is a fixed point that keeps its tokens and comments on shapes nobody wrote; the benchmark generator emits programs that compile at points in its parameter space no profile names; the same input twice says the same thing; and a generated program prints the answer the generator computed — under every backend, and with unreachable code inserted into it. Bounded and seeded in CI; `BURI_FUZZ_SECONDS` soaks. |
| `recovery` | That one mistake reads as one mistake. Every compiling source in the repository, with a token deleted, inserted or exchanged, held to invariants: one diagnostic per mistake, its caret at the mistake, its `fix` naming the token, no type error invented downstream, and the file still formatting. Plus a hand-written case per list context pinning the exact message, span and edit. All six run by default. Each invariant is held per mutation shape. Where the mutated text has a second reading the grammar accepts, it is held against a ceiling rather than against zero, and `ceiling()` states which rows those are and what each residue is. A ceiling is a **percentage of the row's population**, read off a `BURI_RECOVERY_CAP=0` run, so a source landing in the repository cannot tip a row whose per-case behaviour did not change. The two invariants that sample rather than sweep add the spread of a sample that size on top, and `a_ceiling_moves_with_the_row_and_not_with_the_corpus` is that property as a test. |
| `checking` | The same mistakes, pinned rather than counted: seven hundred mutated sources with every error the front end reports about each one recorded beside it. A case whose errors are exactly the parser's lives under `clean/`; one the mistake led the checker into lives under `cascades/`. So a recovery that stops a cascade shows up as a file changing sides, and the share that may cascade is a rate over the corpus. |
| `linting` | What `buri lint` still finds in a file that did not parse whole: each lint fixture's source with one token wrong, through a repository of one package per case, with the whole report recorded. Two invariants over the population, each against a rate ceiling — the mistake invents no finding, and a finding whose evidence survived still fires. The third is a parity rather than a rate: every case compares the set `buri lint` prints against the set the language server publishes, because two halves that go quiet together keep any rate they like. |
| `failing` | That a failing `buri test` fails *well*. It pins the report a user reads — line, expected, got, counts, exit code — byte for byte across every value shape, abort, title edge case and multi-module ordering, so the failure path meets the same standard as the success path. |

Everything but the unit tests drives the real `buri` binary, because that is what
a user runs.

## The trust ordering

Every suite above is worth having, and they are not worth the same. This
section says which one to believe when two disagree, why, and the three rules
that keep the ordering from being a preference nobody enforces.

An incident produced it. `cli/tests/failing/task_order/` and
`cli/tests/failing/every_order/` held the **only** end-to-end coverage of
`tasks().everyOrder()`, the order line and the seed line. A commit renamed
`ctx.parallel(…)` to `tasks.parallel(…)`, which stopped their fixture
compiling. A later commit about a lint blessed both goldens down from eighteen
and fourteen lines to `0 passed, 0 failed, 0 skipped, 1 failed to compile`.
Both cases stayed **green** — `exit:` is hand-written, and a suite that does
not compile exits 1 exactly like a suite whose tests fail — and so did the
runtime-crate tests, the conformance rows and the unit tests under them. Two
waves passed. The lesson is not "watch the goldens". It is that **a lower tier
staying green says nothing about whether the thing a user runs still works**,
and only a tier that runs the whole thing can say otherwise.

### The tiers, most trusted first

| | Tier | What it runs | Where it lives |
|---|---|---|---|
| 1 | **End to end** | A whole program, built by a real backend, in a process of its own, talking over a real socket and receiving a real signal. | `native/e2e.rs`, and the server rows in `native/stencil.rs` and `native/llvm.rs` |
| 2 | **Repository** | A repository on disk, one `buri` command, and what it printed — the CLI a person types, over a package graph, with a linked test binary underneath. | `repositories/*`, `failing/`, driven by `build::repositories` and `failing.rs` |
| 3 | **Conformance** | The same corpus of `test` blocks under every backend, so a divergence is a failure in exactly one of them. | `conformance/`, read by `language::conformance` and `native::conformance` |
| 4 | **Agreement** | One source through two backends, compared byte for byte. | `native/agreement.rs` |
| 5 | **Runtime crate** | `cli/runtime`'s own `#[test]`s: the acceptor, TLS, WebSocket framing, the mailbox, the arena — both ends of a wire, inside one process. | `cli/runtime/*.rs`, reached by `native::runtime` |
| 6 | **Compiler unit** | A function, a hand-built `Program`, a table. | `cli/src/**` `#[cfg(test)]` |

Reading it downwards: **the higher a tier, the fewer of its assumptions are the
test's own.** A unit row asserts about a `Program` a test built out of a list of
strings. A repository row asserts about a `Program` the front end built out of
source somebody could have written. Both are useful, and only one of them notices
when `host.HostListen.listen` turns out not to be a key any real program reaches —
which is what
`e2e::the_refusal_a_toolchain_without_networking_names_what_a_real_server_reached`
found on the day it was written.

Reading it upwards: **the higher a tier, the more of the product one failure
implicates, and the less precisely it names what broke.** That is the trade,
and it is why we demote no lower tier. A red row in tier 5 says which function;
a red row in tier 1 says which program. Keep both, and believe them in that
order.

### What the concurrency-and-servers features have at each tier

The program that added servers, actors, scoped arenas, the `sockets()` double and
four flag days is the worked example. Here is where each of its claims gets
answered.

| Feature | Tier 1 (end to end) | Tier 2 (repository) | Below |
|---|---|---|---|
| serve HTTP/1.1 | `{stencil,llvm}::a_server_answers_a_request_on_a_socket`; `fifty_requests_are_answered_at_once` | `build-files/server_on_a_page` (the graph refusal) | `net.rs`'s acceptor set; `conformance`'s `Scripted` fake |
| TLS, and ALPN choosing `h2` | `e2e::a_tls_port_chooses_a_protocol_and_answers_a_plaintext_client_with_no_http_at_all`; `{stencil,llvm}::a_secured_server_opens_its_port_and_says_why_when_it_cannot` | — | `net.rs`'s `alpn_*`, `http2_*`, `a_certificate_the_runtime_cannot_read_stops_the_bind`; `tls.rs`'s trust-anchor set |
| a protocol the runtime lacks | `e2e::a_protocol_this_runtime_was_not_built_for_is_refused_when_the_port_opens` | — | `net.rs`'s `serves`; `runtime_native`'s `h3` |
| networking absent altogether | *(named exception — see below)* | — | `backend::mod`'s eleven rows, plus `e2e::the_refusal_a_toolchain_without_networking_names_what_a_real_server_reached` over a real `Program` |
| drain on a signal | `{stencil,llvm}::a_signalled_server_answers_the_request_in_flight_and_stops`; `stencil::an_interrupted_server_drains_the_same_way`; `e2e::a_second_signal_ends_a_server_the_first_one_asked_to_drain` | — | `net.rs`'s four drain rows |
| WebSocket lifecycle | `{stencil,llvm}::a_socket_counts_the_messages_it_was_sent`; `llvm::a_broadcast_actor_reaches_a_socket_it_did_not_publish_on`; `e2e::a_full_outbound_buffer_closes_the_socket_and_the_close_hook_is_told_why`; `e2e::an_upgrade_request_reaches_the_request_handler_when_a_server_has_no_hooks`; `e2e::a_websocket_is_served_only_at_the_path_its_hooks_name` | — | `net.rs`'s socket set; `conformance`'s close-code mapping |
| the `sockets()` double | `e2e::the_same_handler_answers_the_same_way_with_and_without_a_socket` | `concurrency/a_room_of_sockets_with_no_server` | `conformance`'s six `sockets()` blocks; `testing.rs` |
| actors | *(a socket hook drives one in the counter rows above)* | `concurrency/an_actor_counts_and_is_stopped` | `conformance/lib/actor/`; `agreement`'s two rows; `rt.rs`'s mailbox set |
| scoped arenas and `copyAcross` | — | `concurrency/a_scope_hands_its_values_out` | `conformance/lib/{actor,memory}`; `agreement`; `memory.rs` |
| the ctx form, and a print that answers a `Result` | — | `concurrency/printing_is_a_result_and_a_context_is_an_argument` | `reject/effect_method_*`, `reject/discarded_result*` |
| import forms across packages | — | `concurrency/three_packages_and_the_imports_between_them`; `libraries/*` | `reject/`, `linting/` |

Two rows are blank at tier 1 and one at tier 2. Each blank is an argument rather
than an omission: they are the exceptions named below.

### The rules

**1. Every feature's happy path *and* its signature failure get a test at the
highest tier that can reach them.** Not a rate, not a sample: the two named
cases. A green happy path says a thing worked on the day somebody wrote it. The
failure beside it says the mechanism is still connected. So `e2e::a_tls_port_…`
proves a TLS port answers TLS *and*, on the same listener a moment earlier, has
nothing to say to a plaintext client. The refusal is only evidence because the
acceptance follows it, and the acceptance is only evidence of a Buri program's
own port because the refusal came first. Every case under
`repositories/concurrency/` ends with the edit that makes its subject fail, and
records the report — the same reason `repositories/`'s own rule already says
*every case that must stay clean ends with the edit that makes it fire*.

**2. A golden can never collapse into the absence of one, silently.**
`harness::no_golden_has_collapsed` runs at the end of every `run_corpus` and at
the end of `failing.rs`'s `recorded_failure_reports`, over the bytes the run
just wrote. It is set equality in **both** directions against
`harness::A_RUN_THAT_ASSERTED_NOTHING`. So a case that starts recording
`failed to compile` or `0 passed, 0 failed` fails, and a case that is
*supposed* to record one and has stopped fails too. The corpus runner calls it
rather than it being a `#[test]` of its own, and that placement is load-bearing:
under `BURI_BLESS=1` a separate test races the blesser and reports the golden it
is in the middle of fixing.

Blessing may rewrite what a report *says*. This is the one thing it may not do.

**3. Every end-to-end test binds loopback, and every wait has a deadline.** The
doctrine is `tls-hang-fix`'s, and the incident behind it is worth keeping
because it was not a flake. A test's server thread sat in `accept()` with no
deadline while the test thread called `join()`, on a host where `localhost`
resolves to `::1` before `127.0.0.1`. Deterministic on every machine that
ordered the two that way, and it took a CI job with it. So:

* the program binds `port: 0` and prints the port; the client dials
  `127.0.0.1`. A test that picks a port and hopes races the pick against the
  bind;
* every connect, read and write carries `shared::SERVER_DEADLINE`;
* **read a reply until it is whole, never to a byte count.** A loop that stops
  at "some bytes arrived" asserts about whatever the kernel coalesced into one
  segment — the whole reply on an idle machine, a head with no body under it on
  a loaded one. `e2e::Until` lets the caller say what a complete answer *is*:
  the peer closed, or one whole TLS record. `e2e::read_loop_tests` is that rule
  with a peer of its own that answers in two writes, so the loaded-runner split
  happens every time instead of on CI only. This is not hypothetical either. It
  took both Linux jobs of run `33539837433`, and the head it printed carried
  the `content-length` of the body it said was missing;
* every wait on a child goes through `shared::waited`, which polls and **kills
  what it could not stop** — `Child::wait` has no deadline;
* join every thread, and read the client's answer *before* joining the server,
  so a client that failed reports as the client failing;
* two things bound a `#[test]` here: the harness's per-invocation hang cap
  (`harness/hang.rs`) for anything it spawns through the CLI, and each CI job's
  `timeout-minutes` outside that.

A broken server is a failing test with a sentence, never a job CI has to kill.

### The exceptions, and why each is one

A tier ordering that pretends to have no gaps is worse than one that names them.
There are three, all in the same place, and all three come from
`language::corpus::dependencies_stay_behind_the_bar`: the workspace may not grow a
dependency for a test, `[dev-dependencies]` included.

**A test cannot use a TLS or WebSocket library.** The only `rustls` and the
only `tungstenite` here sit inside the runtime archive. So the protocol-level
claims — the handshake, multiplexing, masking, fragmentation, the close
handshake — live at tier 5, in `cli/runtime/net.rs`'s own tests, where both
ends of the wire are reachable. Tier 1 asserts the half only it can: that a
*Buri program's* `Server` opened the port that answered. Its clients are
hand-written and deliberately minimal. `e2e::client_hello` is the smallest TLS
1.2 `ClientHello` a `rustls` server will answer, at 1.2 precisely because 1.3
encrypts the ALPN answer and reading it would need the library that is not
here. `shared::Talking` is one masked text frame at a time.

**A repository fixture cannot run a server.** `repositories/` reaches a native
backend through `buri test`, and a test source may not import `core/host` (SPEC
4.1.1: only the module exporting `main` may). So a repository case can assert
the *graph* refusal — `build-files/server_on_a_page` — and anything that runs a
listener sits at tier 1, in a process of its own.

**`networking-not-available` has no whole-process row.**
`runtime_native::net()` reads a file `cli/build.rs` writes beside the archive
and `include_str!` bakes into the binary. So *running* that refusal would mean
building `buri` a second time with `BURI_RUNTIME_NET=0` — minutes of `cargo`,
inside a test.
`e2e::the_refusal_a_toolchain_without_networking_names_what_a_real_server_reached`
stands in its place. It drives the real front end over a real
`server.bind`-and-`run` source and asks the refusal's own seam what it would
say, recording the eight intrinsic keys such a program actually reaches — the
half a hand-built `Program` could never have got right.

None of the three is a hole a lower tier quietly covers for. Each is a sentence
about what the top tier cannot reach, written where a reader looking for the
missing row will find it.

## Running them

```
cargo test -p buri                                    # everything
cargo test -p buri --test language                    # one domain
cargo test -p buri --test language conformance::      # one suite in it
cargo test -p buri --test native -- --skip float_parity
cargo test -p buri --features backend-llvm --test native
BURI_RECOVERY_CAP=0 cargo test -p buri --test recovery   # every case, not a stride
```

The first line is the whole suite, and the `release` job runs exactly that. The
`test` legs run **the same set with its binaries overlapped**, in four lines of
inline shell in `.github/workflows/ci.yml` rather than a script:

```
cargo test -p buri --no-run --message-format=json-render-diagnostics \
  | jq -r 'select(.profile.test == true) | .executable | select(. != null)' > units.txt
xargs -P "$(( $(getconf _NPROCESSORS_ONLN) * 2 ))" -n 1 \
  sh -c 'exec "$0" --test-threads=2' < units.txt
```

`cargo test` starts the test binaries **one after another**. That is what the
command does, not a knob. The suites here are latency-bound rather than
core-bound, so most of the machine sits idle for most of the run. On a
four-core runner that is the whole of the longest job's wall clock: measured on
run 33981313436's arm64 leg, the step took 424 s, of which **145 s was
compiling and 280 s was running fifteen binaries in a queue**, the longest of
them 54.5 s. Started together on a ten-core mac the same fifteen take **83 s
against 165 s**, for about 400 CPU-seconds of work either way.

Two numbers in that snippet are measured rather than tidy. `--test-threads=2`,
because a core with one runnable thread on it idles through the `cc` and
child-process stalls the second thread hides. **Twice** `nproc` of workers,
because of the order cargo emits its artifacts in: `native` is the longest unit
by a factor of two and it comes fourteenth of fifteen. At `nproc` workers it
does not start until the other thirteen have been dealt out, and it then holds
the run open alone. Sorting the list is the other fix, and we do not take it.
Neither binary size nor test count predicts a unit's runtime here — the largest
binary takes 6 s and the smallest takes 26 — so any order worth having would be
a hand-written list of domain names, and
`ci.rs::the_suite_is_asked_for_as_a_whole` forbids one. The set is cargo's
answer, so a domain added tomorrow runs the day it is added.

Merging a domain costs nothing in selection. A module is a name prefix, so
`--test language conformance::` runs exactly what `--test conformance` used to,
and `--skip` takes a module out of a run the same way.

Every suite works on a copy under `CARGO_TARGET_TMPDIR`. Nothing writes into a
checked-in tree, so the suites hold no lock, run in parallel, and two
`cargo test` runs in two shells do not collide. `BURI_KEEP=1` leaves the
scratch directories behind, and a panicking test leaves its own regardless: a
failing test's evidence is the directory it failed in.

A run also sweeps that directory once, before it makes its first tree
(`harness/sweep.rs`). Most scratch removes itself when its `Scratch` drops. The
native suites' per-process trees — `native-stencil-<pid>` and its siblings,
about 180 MB a run, holding a runtime archive and a hundred linked executables
— are named for the process so two overlapping runs cannot share one, and
nothing deletes them. Fourteen gigabytes of them filled a disk mid-measurement,
twice. The sweep takes only what has not been written to for **two hours**,
which no live run can manage, and it does not run at all under `BURI_KEEP`. So
the contract above holds unchanged for the run that produced the evidence, and
for hours after it.

### Reproducing a Linux CI leg on a mac

No script exists for it any more, because there is nothing left to script. A
`test` job is a container with the toolchain in it, then the same
`cargo test -p buri` typed above. The container needs two things: `CC=clang`
and the musl `rust-std`. `cli/build.rs` degrades silently without either, and
`ci.rs`'s liveness gates say so before the suite spends ten minutes proving
nothing:

```
docker run --rm -it -v "$PWD":/w -w /w rust:latest bash -c '
  apt-get update &&
  apt-get install -y --no-install-recommends clang lld mold llvm binutils musl-dev musl-tools &&
  rustup target add "$(uname -m)-unknown-linux-musl" &&
  CC=clang BURI_CI=1 cargo test -p buri'
```

`--platform linux/amd64` on the same line runs the x86-64 leg under emulation.
It works, and it is slow enough to be an overnight answer rather than an edit
loop. `BURI_CI=1` turns a guard that fires into a failure rather than a quiet
pass, which is the whole reason to run the leg at all.

### Skips: none on CI, and each one on a host has a name

A test that does not run proves nothing and costs what a running one costs to
compile. A test can fail to run here in three ways, and each has an answer.

**`#[ignore]`.** There are **none** in the whole repository, and
`.github/known-skips.txt` is the empty list that says so. It shipped with two
rows, the two agreement rows parked on missing compiler features, and both came
out by somebody writing the feature rather than deleting the row:
`derivePrimJson` has a native body on both backends, and
`host.HostAlloc.allocate` has a runtime row.
`cli/tests/ci.rs::the_only_ignored_tests_are_the_ones_named_here` walks the
tree and fails if the set it finds is not exactly that file's, so nobody adds
the first new one quietly, on a runner or on a laptop. In order of preference:
fix it; `#[cfg]` it out on the host that genuinely cannot answer it, so it is
absent rather than reported as not run; or delete it, if nobody wants the
behaviour it asserts. A row in that file is the last resort, and it is a named
defect rather than a permission.

**`if !supported() { return; }`.** The native suites open with one, and it is
load-bearing on a host with no C compiler or no stencil library for its triple:
such a machine should get a suite that says so rather than a wall of red. On CI
it is the one shape of green this repository refuses, because every runner
installs the tools and asserts the stencil libraries and the runtime archive
are real bytes before the suite starts. So `BURI_CI=1` — set in the workflow's
`env:` block, and therefore in every job — makes `harness/ci.rs::skipped` PANIC
instead of returning. Set it locally to see what a runner sees.

**Deferrals.** `repositories::language_server_speed` and
`repositories::language_server_open_cost` assert milliseconds rather than work, so
they return early unless `BURI_PERF` is set, and they mean nothing outside
`--release`:

```
BURI_PERF=1 cargo test --release -p buri --test build repositories::language_server_
```

Both hold every editor request to 50 ms. CI runs them on its arm64 runner
(`.github/workflows/ci.yml`, `language-server-budget`), where
`BURI_PERF_BUDGET_SCALE` widens the bar for a machine slower than the one the
number came from. They say so through `ci::deferred_to`, which names that job,
and `cli/tests/ci.rs::every_deferral_names_a_job_that_still_asks_for_it` holds
the name to a job that exists. A deferral whose job has been renamed is a plain
skip, and nothing else would notice.

### The runtime crate's tests are run by a test

`cargo test -p buri` cannot reach the `cli/runtime` cargo package, so
`native::runtime::the_runtime_crate_answers_its_own_tests` runs its ninety-seven
assertions instead. It shells a nested `cargo test` against the package
`cli/build.rs` assembles in `$OUT_DIR`, with the same features the archive beside
this binary was built with. That cold-compiles tokio and rustls the first time —
about ten seconds here, a minute on a cold runner — into a target directory under
`CARGO_TARGET_TMPDIR`, and once per checkout after that.

It was a workflow step once, with a cache key and a stamp file the test read
instead of running anything. The step is gone. A test that asserts a receipt is
not the same test as one that asserts a result, and the arrangement cost a
hundred and fifty lines of bash to save a minute on a runner.

Neither fails on a single reading. A run holding a request over the bar
measures its whole session again — a fresh server against a fresh copy of the
repository, up to three times — and holds each request to the fastest time it
was seen in (`best_of`). A request that got slower is slower every time and
still fails; one that lost a timeslice on a shared runner is not. The
measurement repeats, not the assertion: the bar applies once, to the best
readings, and it does not move.

### What the run costs

Everything but the unit tests drives the real `buri`, so **the toolchain
compiles itself out of a `cargo test` run**, and an unoptimized compiler makes
a slow suite rather than merely a slow build. `Cargo.toml` therefore puts
`buri` and the test binaries at `opt-level = 1` in the `dev` and `test`
profiles. That is worth about fifteen seconds a run, costs nothing measurable
to build, and keeps `debug-assertions` on. The scoping used to do a second job
— leaving the removed debug backend's dependency closure unoptimized, because
no suite waited on it — and lost it when that backend went
(`design/native/CODEGEN-STENCIL.md` §13). A default build now has nothing in
its dependency closure to scope around.

What is left is `native`, and the host binds it rather than the toolchain. It
links and executes about a hundred and twenty freshly built binaries, and macOS
charges roughly 200 ms to execute a binary written a moment ago, however small.
That is most of the domain's wall time on a mac and almost none of it on Linux.
`--skip float_parity` is not where the time is.

We measured that the host costs this rather than asserting it. `native`'s wall
clock is flat at 40.7, 39.1 and 38.1 seconds under `--test-threads=4`, `10` and
`20`. Five times the threads buys 6 %, because the threads wait on `cc` and a
child process rather than on a core. So "more threads inside a binary" is not a
lever here, and neither is making the compiler faster: a whole front end plus
monomorphization plus `middle` over the standard library and a conformance file
measures **≈ 20 ms**, against **≈ 400 ms** to emit, link and run that same
file.

**The lever that observation does point at is one binary out.** A domain using
1.5 cores of a ten-core machine for a minute does not want more threads. It is
a minute during which fourteen other binaries could have been running and were
not, because `cargo test` starts them one at a time. Same tests, same threads,
same CPU-seconds: **165 s queued, 83 s overlapped** here, and 280 s queued on
the four-core arm64 runner. We pull that lever in four lines of inline shell in
the `test` legs rather than in a script — "Running them" above carries the
snippet and the two numbers in it. It takes four lines instead of two hundred
and seventy because the three CI assertions that used to parse the concatenated
log are tests in `ci.rs` now, so the runner only has to start the binaries and
keep each one's log readable.

### The five-minute budget

The whole verification bar runs in **under five minutes**. That is a policy
rather than an observation. A change that pushes it over owes either an
optimization that brings it back or a written justification beside the change.
The ledger of past offenders says the optimization is usually there to find: an
exponential scan in `rc.rs` was once 96 % of an 829-second run, and fixing it
also fixed a product bug.

Two rules keep the budget honest. **Coverage never pays for it.** The way back
under the line is faster mechanics — build profiles, caching, shared analysis,
deduplicating a binary that literally re-runs tests another suite already ran —
and never running less. And **the number is measured, not asserted.** A change
that touches this suite's cost reports the bar's wall time against the budget
in its verification section.

#### What "the bar" is

A budget with no canonical command is a budget nobody can reproduce. Here is the
sequence, and it is the one the number below came from:

```
cargo test -p buri
cargo test -p buri --features backend-llvm --lib compiler::backend::llvm::
cargo test -p buri --features backend-llvm --test native -- llvm:: agreement:: e2e::
cargo test -p buri --features backend-llvm --test fuzz
cargo bench  -p buri --bench compiler --profile validate -- --validate
cargo clippy -p buri --all-targets
cargo clippy -p buri --all-targets --features backend-llvm
```

`LLVM_SYS_211_PREFIX` has to be set for the three feature lines and for the second
clippy. The fuzz binary runs **twice**, once in the first line and once in the
fourth, and `--check-reproducible` runs inside `build`'s forty-nine, so neither
needs a line of its own.

**Why the feature leg is three lines rather than one.** A plain
`cargo test -p buri --features backend-llvm` runs 917 tests, and 843 of them
are the ones the first line just ran, with the same code, to the same answer.
The delta is 74 tests by name — fifteen `backend::llvm` unit tests and the 59
in `native::llvm` — plus three suites whose existing tests do more work under
the feature rather than appearing as new ones. `native::agreement` and the
whole `fuzz` binary each keep a `NATIVES` table that *gains* an `llvm` row, and
`native::e2e` builds each of its whole programs with whichever native backend
the toolchain has, which is the copy-and-patch one on a default build and LLVM
here. Those five selections are the delta, and running them instead of the
other 788 is **dedup, not less coverage**.

It is only dedup while the delta stays where we say it is, so that is a test.
`language::corpus::the_llvm_feature_is_confined_to_the_files_the_bar_names`
reads every `.rs` file under `cli/src` and `cli/tests` and fails if a
`cfg(feature = "backend-llvm")` appears outside those files. One hole it cannot
cover belongs here, honestly: `backend::select` answers with LLVM for a *native
release* build, so a test driving one through the CLI would differ under the
feature with no `cfg` of its own. Every `--release` in the suite today builds a
`platform: JS` output, and `native_ready` rejects `Js` before anything reaches
`select`. But a change that adds a native `--release` test owes this paragraph
a second look.

**One test now takes that second look on purpose**, named here because the
paragraph above asked for it.
`compiler::backend::a_release_refusal_names_the_profile_rather_than_the_platform`
asks `build::actions::native_gap` for the host's own target under
`Profile::Release` and asserts *both* answers: a refusal naming `backend-llvm`
on a default build, and no gap at all under the feature. It drives no CLI and
builds nothing, so it costs the same on both legs. What it does is pin the
sentence buri-lang/buri#26 was about, which exists on only one of them.

**CI is not this.** CI runs everything under both feature sets, on both hosts.
The sequence above is the local edit loop, and the only thing it takes away is
the same run happening a second time on the same machine a minute later.

One line of it *is* CI's: the validation gate runs under `--profile validate`
there too. That is the profile the root `Cargo.toml` declares for exactly this.
Validation under `[profile.bench]` cost a fat-LTO, one-codegen-unit build of
the whole toolchain to compute a boolean, and CI was the last caller still
paying it.

#### The measured number

Sequentially, on a quiet ten-core M-series mac (8P + 2E, macOS 25.5, the nix
devshell), against `d0d83ff`:

| leg | warm | after a `cli/src` edit |
|---|---:|---:|
| `cargo test -p buri` (843 tests) | 71.1 s | 80.1 s |
| the three `backend-llvm` lines (129 tests) | 39.8 s | 50.1 s |
| `-- --validate` | 10.6 s | 36.6 s |
| clippy, both feature sets | 0.3 s | 8.9 s |
| **the bar** | **122.0 s** | **175.9 s** |

Two warm runs, 122.0 s and 128.1 s. The slower one had clippy's cache cold for a
file the run before had just edited, which is 2 s of the 6.

"After a `cli/src` edit" is the column that matters, because it is the loop. The
compiler rebuilds, ten test binaries relink, and `--validate`'s `opt-level = 3`
build has no incremental cache to fall back on — 26 of its 36.6 seconds all by
itself.

Cold, in the sense that matters — `cargo clean -p buri`, so every artifact of
this crate rebuilds for both feature sets and for clippy twice, with the
dependency graph still cached — the bar is **253 s**. Add
`[profile.validate]`'s own 26-second build, which `cargo clean -p` does not
reach, and it is about **279 s**. Still inside the line, with the build four
fifths of it.

We do not measure a *fully* cold bar in an empty `CARGO_TARGET_DIR`, and the
reason is worth keeping: it needs on the order of fifteen gigabytes, and this
directory has filled a disk mid-measurement twice already. It is also not the
budget. The budget is the loop an edit actually runs, which is the second
column above — a warm target directory and an edited `cli/src`.

Two entries in that table are worth naming, because we bought both rather than
found them. `--validate` is 10.5 s instead of 11.7 s *plus a 169-second
link-time-optimized build*, because it runs under `[profile.validate]` rather
than `[profile.bench]`; the root `Cargo.toml` states why that cannot change a
verdict. And the feature leg is 41.8 s instead of the 94 s a second full suite
costs, which is the dedup above: **56 seconds a run, for zero tests.**

#### Things that were priced and are not worth doing

Recorded so we price them once rather than propose them again. All three were
worth minutes before somebody fixed `rc.rs`'s exponential scan. That fix took
the pipeline they all target from seconds to milliseconds, and took them with
it.

The last row is the one we *did* do, and it sits here rather than among the
timings above because of what it did **not** buy. A census that halves and a
suite that does not move is what "already hidden behind the other tests" looks
like from outside. libtest runs ten of these at once, the census's walk spent
its time blocked on `cc`, and the other nine threads were never waiting for it.
Worse, the cores the batch takes come out of the test that *is* the binary's
critical path: measured, `--test native` is 37.3 s with the census on two
workers and 42.9 s on four, and that is what pins the width in
`stencil.rs::each`.

The lever that is actually large in that binary is
**`conformance::the_native_set_passes`, 41 s of a 36-second `--test native`**
by nextest's per-test clock, doing the identical walk: thirty-six corpus files
compiled, linked and run one after another in one thread. The mechanism in
`stencil.rs` is the shape that would batch it.

| lever | measured | |
|---|---:|---|
| Drop the duplicate pipeline in `conformance::the_native_set_passes` (it ran the front end twice per file) | **0.15 s** of that test's 10.9 s, and 0 s of `native`'s 38 s | implemented, measured, reverted |
| Share one `SourceMap` + `parser::Cache` across the corpus walkers | bounded above by the one above: a whole pipeline is ~20 ms, and this removes only its parse | not attempted |
| Batch the two stencil corpus tests onto one emit | `the_corpus_census_is_a_ratchet` is **0.556 s** in total | superseded by the row below |
| Batch the stencil corpus census into one child, walked two files at a time | the two census tests are **15.0 s -> 8.1 s** run on their own, and CI's liveness step (the ratchet alone) **1.69 s -> 0.92 s**. `--test native` is **about a second *worse***, and `cargo test -p buri` does not move at all | implemented |

## Why each shape exists

**The conformance repository** is ordinary Buri. Its packages are libraries
whose `test/` directories hold `test "..." { ... }` declarations, so each case
exercises the whole pipeline — parse, check, monomorphize, emit, minify, run —
and asserts on a value rather than on an exit code.

`lib/canary` keeps the suite honest. `conformance_suite_can_fail` rewrites a
constant in it and asserts the runner notices. A suite that cannot fail is not
evidence.

**Rejection and abort corpora** cover what assertions cannot. A program that must
not compile has no runtime to assert in, and nothing can catch an abort — the
language has no `catch` — so both get a harness that compiles or runs the program
and checks the diagnostic.

Overflow used to live in the abort corpus. It is undefined behaviour now (SPEC
6.2), so there is nothing to assert. Those files were deleted rather than
weakened into tests of whatever the backend happens to do.

**The formatting corpus** is a directory per decision the formatter makes,
holding an `input.buri` somebody might have typed and the one `expected.buri`
it is allowed to produce. There is no third file, because there is nothing to
configure. A formatter with options has no single right answer, and this suite
exists to say that this one does. `formatting.rs` also holds every output to
being a fixed point, to keeping the comments and tokens it was given, and to
fitting the margin — the last except in the `width_*` cases, named for the
atoms that cannot break. "Every output is a fixed point" is also the claim that
the formatter leaves well-formatted source alone, made over the whole corpus
rather than over a handful of files chosen to say it. A `NOTES.md` beside a
case marks a shape that is pinned rather than endorsed; there are none today.

```
BURI_BLESS=1 cargo test -p buri --test formatting
```

A case named `textproto_*` is a build file rather than source. `buri format` has
two printers and this corpus pins both, and
`every_checked_in_build_file_is_formatted` holds the repository's own two hundred
`BUILD.buri` and `REPO.buri` files to what the second one prints.

A case named `recovery_*` is source with a **syntax error** in it, and it pins
what the formatter does about that: the declaration the parser could not read
comes back byte for byte, and everything around it gets laid out. Every other
case's input has to parse, and the harness says so rather than quietly
formatting a broken file. The four claims hold for these too, restated where a
broken file makes them. The output is a fixed point. It keeps every comment and
token. It carries the same number of syntax errors as its input, with every
region byte for byte what was written. And every line the formatter *laid out*
fits the margin — a line inside a region is the author's, the same argument
`width_*` makes.

This corpus sits outside the repository-wide walkers on purpose. An
`input.buri` is misformatted deliberately, and a suite asking whether every
source in the repository is already formatted would be asking these files a
question they exist to answer no to.

**The generated corpora** are the same bargain at three orders of magnitude,
and a program writes them rather than a person typing them.
`formatting/generated/` holds a thousand `recovery_*` pairs, `checking/` seven
hundred sources with the page the front end prints for each, and `linting/` six
hundred lint fixtures with the findings they still draw. `harness/pinned.rs`
samples all three from `harness/mutation.rs`'s population, one case per
**coverage cell**: the mutation kind, the delimiter open at the site, the
declaration around it, what opened that delimiter, and the tokens either side.
Two mutations in one cell are the same test, so a cell keeps at most two and
the smallest seed that exhibits it wins.

Every one of them regenerates, and each suite has the test that says so: run
the sampler again from the same seed over the same sources and it chooses
exactly the checked-in cases and writes exactly their bytes. Blessing is
therefore idempotent, and one command regenerates all three:

```
BURI_BLESS=1 cargo test -p buri --test formatting --test checking --test linting
```

One constant per suite holds the count — `GENERATED_TOTAL` and `TOTAL` — so
scaling the corpus is a number and a bless.

Each file carries its expectation on its first line:

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

The `// EXPECT:` line says what the case is *about* in one phrase. The two
recorded files pin what a user actually reads: span, carets, notes, the order
of several diagnostics, every word of the prose. A reworded message changes the
product, so it should turn up as a diff somebody looks at, rather than pass
silently because a substring happened to survive. After a deliberate change:

```
BURI_BLESS=1 cargo test -p buri --test language conformance::rejected_programs
```

The JSON file also enforces the four-part contract: **every diagnostic must
carry a `fix`**, and the harness fails the case if one does not. That is a rule
about the product, checked case by case rather than asserted in prose.

Recording these found two bugs a substring check never could. The
`did you mean` suggestion picked an arbitrary winner among equally-close
candidates, so the same compiler on the same file printed `Add`, `Ord`, or `Eq`
depending on hash order. And the `= ...` lines sat one column right of the `|`
gutter above them, which nobody notices until the output is a file you diff.

**The fuzz corpus** is the one corpus nobody wrote. A search found every case
in it, minimised it until nothing more would come out, and wrote it down so the
finding cannot be lost:

```
cli/tests/fuzz/generated_derive_with_an_empty_trait_list/
  CASE.textproto      doc, which property failed, and whether it is still open
  input.params        the minimised input — `input.buri` for the five
                      properties whose input is source
```

`status` is what lets a fuzzer live in a suite that has to stay green. `FIXED`
is the ordinary regression: the property must hold. `OPEN` is the reverse — the
property must **still** fail — so a known-open finding is pinned rather than
quarantined, and the day somebody fixes it the suite fails and says to move the
case to `FIXED`. Swift's `compiler_crashers` / `compiler_crashers_fixed` split
is the same idea, and so is rustc's `//@ known-bug:` header. Both buy the same
thing: somebody *notices* when an unrelated change accidentally fixes a bug.

The searches skip a finding whose signature the corpus already holds, so an
open bug does not hide every bug behind it. That is the failure mode a fuzzer
in CI has and a corpus does not.

```
BURI_FUZZ_SECONDS=600 cargo test -p buri --test fuzz     # soak
BURI_FUZZ_RECORD=1 …                                     # write findings down
```

Recording is opt-in because nothing else here writes into a checked-in tree, and a
suite that silently grew a corpus on every CI run would be the loudest possible
exception to that.

**The repository corpus** exists because the two corpora above cannot hold a
build-system test. A reject case is synthesised as a single-package binary with no
dependencies, so nothing in it can express `missing-dep`, `dep-cycle`,
`visibility-violation`, or a tag conflict — which is most of what the build system
checks. A case there is a repository instead:

```
cli/tests/repositories/build-files/missing_dep/
  CASE.textproto      the manifest
  repo/               the repository, copied into a scratch tree and run in
  expected/lint.txt   what the CLI printed, recorded
```

The manifest is textproto, the format `REPO.buri` and `BUILD.buri` already use,
read by the toolchain's own parser. It deliberately does *not* end in `.buri`:
only the tree under `repo/` is a Buri repository, and a file the toolchain
never reads should not wear the extension that says it does.

```
doc:  "one line saying what the case is about"
run  { args: ["lint", "//cmd/app"]  exit: 1  golden: "lint.txt" }
run  { args: ["build"]  exit: 0  cwd: "lib/money" }
edit { file: "cmd/app/BUILD.buri"  replace: "..."  with: "..." }
file { path: "cmd/f/main.buri"  golden: "formatted.buri" }
path { path: ".buri/out"  exists: false }
path { path: "out"  symlink: ".buri/out/js" }
```

`cwd` runs the command from a directory inside the repository, which is the
only way to ask what a command with no target operates on. `path` covers the
commands whose contract is about what they leave on disk rather than what they
print. `clean --outputs` and the `out/` symlink say almost nothing, and an exit
code cannot tell a cache that survived from one that was deleted and rebuilt.
Exactly one expectation per `path` step, spelled out: like `exit`, an assertion
here is never inferred.

`exit` is hand-written and required. Only prose gets blessed, so blessing can
rewrite what a diagnostic *says* and can never quietly turn a rejection into an
acceptance — a flipped exit code fails a `BURI_BLESS=1` run too.

Steps run in order against one scratch copy, so a case shows a rule firing and
then shows that the fix the diagnostic printed actually works. **Every case
that must stay clean ends with the edit that makes it fire.** A positive result
is only evidence when its negative twin sits next to it and cannot drift away.
`visibility_skips` is the shape to copy: a private library reached by its own
test suite and its co-located binary, clean, and then one edge from outside.

Goldens stay path-stable without scrubbing, because every file enters the
source map under a repository-relative name. So `--> cmd/app/main.buri:9:6` in
a recorded file is also a path you can open.

A case about *the platform this toolchain is not* cannot write the platform
down. `linux` names a target a mac refuses and names the host on a Linux
runner, so a golden holding the word would be true on one machine and false on
the next. Such a case writes a placeholder instead:

```
run { args: ["build", "//cmd/native", "--output={{CROSS_PLATFORM}}/{{CROSS_ARCH}}"] exit: 1 }
{ platform: {{CROSS_PLATFORM_PROTO}}, arch: {{CROSS_ARCH_PROTO}} },   # in a BUILD.buri fixture
error: the {{CROSS_PLATFORM}} backend is not implemented              # in a golden
```

The harness fills these in from a table keyed on the host — `linux/x86_64` on a
mac, `macos/x86_64` on Linux — in the fixtures it copies into the scratch tree,
in the manifest's own strings, and in reverse on the way back out. So what gets
compared against a golden and what `BURI_BLESS=1` records both hold the
placeholder. It is the same trick `<scratch>` already plays with the temporary
path, and it means blessing on either machine writes the same file.

The pair guarantees the toolchain refuses it. `native_ready` ends in
`link.rs::can_link`, which is host-only, because the runtime archive the binary
embeds is built for the host. The two hosts' spellings are the same width on
purpose: a caret run is as wide as the line it underlines, and no placeholder
can stand in for one. The harness substitutes only the facts a case actually
names, so every other case's goldens come back byte for byte.

**The incrementality suite** reads `buri build --explain`, which prints one
line per action with its key and whether the cache served it. The claims in
`buri docs build/hermeticity` are about *which actions run*, and until that
flag existed nothing outside the toolchain could observe one. It compares keys
between two states of one tree and never records them, because a key includes
the toolchain version and a recorded one would move on every release.

**The golden transcript** catches a backend that produces a *different* answer
rather than no answer, on the one path no assertion inside a program can reach:
what a whole program prints on its way out. It is one line, so it lives as a
literal in the test rather than in a file — a change to what `//cmd/web` prints
should be read in the diff, not blessed. The conformance suite asserts
everything else about rendering — `${}` interpolation of every type, float
formatting, captured stdout — where a wrong answer is a failed `assert.eq`
rather than a diff.

**The emitted-JavaScript corpus** is the only suite that looks at the output
rather than the answer. Every other suite here is blind to an optimisation by
construction: removing an allocation, a call frame or a redundant test changes
no value anywhere. So without a record of what the backend emits, a pass lands
unseen and regresses unnoticed. Each case is one small program exercising one
construct:

```
cli/tests/golden_javascript/enum_match/
  main.buri       the program
  expected.mjs    the generated code, with the runtime removed
  expected.out    what it prints
cli/tests/golden_javascript/sizes.txt   release artifact sizes, whole corpus, one file
```

The runtime comes out because it is the same thousand lines in every case and
no pass changes it. What is left is exactly what the backend produced, with
debug names, so the diff reads well. `sizes.txt` is one file rather than one
number per case, so a change's size effect shows up as a single diff. Each case
must be smaller in release than in debug.

**`expected.out` is recorded once and never re-recorded.** `expected.mjs`
records *how* a program compiles and is meant to move with every pass.
`expected.out` claims what it *computes*, and no pass may move it. So
`BURI_BLESS=1` writes it when it is missing and refuses to overwrite it
afterwards. To change one deliberately, delete it and re-record. Every case
also runs in both build modes, and the two must print the same bytes.

This is not hypothetical. A parallel-move optimisation in the tail-call
rebinding read a parameter after overwriting it, turning a sum of `5050` into
`4950`. The conformance suite has no tail-recursive accumulator of that shape,
so this corpus was the only thing that saw it — and had `expected.out` been
blessable, blessing would have recorded the wrong answer and moved on.

```
BURI_BLESS=1 cargo test -p buri --test language golden_javascript::
```

Blessing without reading the diff is the one way this suite proves nothing.

## Properties pinned outside the corpora

`language/conformance.rs` also holds the checks about the toolchain rather than
the language:

- **Tail calls run in constant stack on V8.** Ten million bounces through a
  self-recursive function and a mutually recursive pair, on an engine with no
  native proper tail calls. This is the test that says the compiler does the
  elimination itself rather than leaning on JavaScriptCore.
- **`--release` and `--debug` agree.** Mangling, dead code elimination,
  constant folding and runtime tree-shaking may not change what a program
  computes or prints. The whole conformance suite runs both ways and has to
  pass the same number of assertions, and the monorepo's binary has to print
  the same bytes. The release artifact also has to be *smaller*, so nobody can
  buy identical behaviour by doing nothing.
- **Builds are reproducible.** Copy the worked monorepo to two different
  directories, build it in each, and the output is byte-identical — so neither
  a path nor a hash-map ordering leaks in.
- **The cache cannot serve a stale answer.** Build, edit, rebuild, and check
  the program's *behaviour* changed. Then revert and check the original entry
  comes back.
- **Exit codes distinguish bad code from a bad invocation**, which is the
  distinction CLI.md draws between 1 and 2.
- **`format --check` reports without rewriting**, and formatting is a fixed
  point.

`native/agreement.rs` is the one such check outside `language/conformance.rs`,
and it is about the *pair* of backends rather than either one. It compiles a
single `.buri` source through `actions::prepare` and `backend::select` twice —
JavaScript under `bun`, native through the copy-and-patch backend or LLVM and
`cc` — then compares the two outputs byte for byte. Every row of
`design/native/VALUE-MODEL.md` §12 is a `#[test]`, so a failure names the row,
and `every_row_of_the_table_names_a_test_that_exists` reads the table back and
fails on a row whose test is missing. A row the native surface cannot reach yet
gets a gap test naming the missing intrinsic *and* an `#[ignore]`d agreement
test beside it, so neither can rot alone. It skips with a printed reason where
`native_ready` is false or no JavaScript engine is on the path, and compiles to
nothing with `--no-default-features`. A backend with no seat on this *host* —
stencil on a machine whose `cc` built no library, or on macOS x86-64, which
none is built for — is left out of the row by name and by reason. That reason
comes from `stencil::unavailable_reason`, so the rows light up the day the seat
lands.
