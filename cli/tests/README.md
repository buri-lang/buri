# Testing the toolchain

A compiler that exits 0 has proved nothing. These suites are arranged so that a
wrong *answer* fails, not merely a program that fails to run.

| Suite | Where | What it proves |
|---|---|---|
| Unit tests | `cli/src/**` (`#[cfg(test)]`) | The lexer, parser, textproto reader, type unifier, JS printer, minifier, SHA-256, and SCC finder do what they claim in isolation. |
| Corpus | `cli/tests/corpus.rs` | Every `.buri` file in the repository that is meant to compile parses; every build file reads; formatting is a fixed point. |
| Standard library | `cli/tests/standard_library.rs` | `core/*` typechecks against itself — the first program the compiler ever sees. |
| Conformance | `cli/tests/conformance/` | A Buri repository whose `test/` directories assert on language semantics, run through the real `buri test`. |
| Rejection | `cli/tests/reject/` | Programs that must **not** compile, one directory each, holding the diagnostics exactly as the terminal and `--error-format=json` print them. |
| Abort | `cli/tests/crash/` | Programs that must compile, then abort, saying why. Division by zero, a shift past the width of its type, an empty random range. |
| Repositories | `cli/tests/repositories/` | Whole repositories, one per build-system rule, each with a manifest of what the CLI does in it and the output that produces. |
| Native runtime | `cli/tests/runtime_native.rs` | The `buri_rt_*` C ABI, from C: that the archive links, that the 16-byte reference-count header and drop-glue dispatch behave and leak nothing, that every abort message is byte-identical to the JavaScript backend's, and that each host capability — the two output streams and their interleaving, standard input's two forms, the filesystem, the environment, the clock, randomness, and an HTTP fetch against a socket the test owns — answers what `core/cap` declares. |
| Incrementality | `cli/tests/incrementality.rs` | What the cache may and may not do, read off the `--explain` transcript. |
| Hermeticity | `cli/tests/hermeticity.rs` | That an action's spawn is deterministic and a perturbed parent environment changes neither an artifact's bytes nor a suite's verdict; what two builds of one tree have to agree about; and that four concurrent builds leave the cache intact. |
| Emitted JavaScript | `cli/tests/golden_javascript/` | What the backend *compiles to*, one construct per case: the generated code, what it prints, and the release size of the whole corpus. |
| Golden | `WEB_STDOUT` in `conformance.rs` | The exact stdout of the worked monorepo's JS binary. |
| Protobuf conformance | `cli/tests/proto/` | Protobuf's own conformance suite against the codecs generated from a `.proto` schema — the one test here whose ground truth comes from another project. Driven by a C++ runner, so it lives outside `cargo test`; `cli/tests/proto_vectors.rs` replays recorded exchanges without it. |

Everything but the unit tests drives the real `buri` binary, because that is
what a user runs.

## Running them

```
cargo test -p buri                       # everything
cargo test -p buri --test conformance    # the language suites
cargo test -p buri --test repositories          # the build-system suites
```

Every suite works on a copy under `CARGO_TARGET_TMPDIR`. Nothing writes into a
checked-in tree, so the suites hold no lock, run in parallel, and two
`cargo test` runs in two shells do not collide. `BURI_KEEP=1` leaves the
scratch directories behind, and a panicking test leaves its own regardless —
a failing test's evidence is the directory it failed in.

## Why each shape exists

**The conformance repository** is ordinary Buri. Its packages are libraries
whose `test/` directories hold `test "..." { ... }` declarations, so the thing
being exercised is the whole pipeline — parse, check, monomorphize, emit,
minify, run — and the assertion is on a value, not on an exit code.

`lib/canary` exists to keep the suite honest: `conformance_suite_can_fail`
rewrites a constant in it and asserts the runner notices. A suite that cannot
fail is not evidence.

**Rejection and abort corpora** cover what assertions cannot. A program that
must not compile has no runtime to assert in, and an abort cannot be caught —
there is no `catch` in the language — so both get a harness that compiles or
runs the program and checks the diagnostic.

Overflow used to live in the abort corpus. It is undefined behaviour now
(SPEC 6.2), so there is nothing to assert: those files were deleted rather than
weakened into tests of whatever the backend happens to do.

**The formatting corpus** is a directory per decision the formatter makes,
holding an `input.buri` somebody might have typed and the one `expected.buri`
it is allowed to produce. There is no third file, because there is nothing to
configure: a formatter with options has no single right answer, and this suite
exists to say that this one does. `formatting.rs` also holds every output to
being a fixed point, to keeping the comments and tokens it was given, and to
fitting the margin — the last except in the `width_*` cases, which are named
for the atoms that cannot break. That every output is a fixed point is also
the claim that well-formatted source is left alone, made over the whole corpus
rather than over a handful of files chosen to say it. A `NOTES.md` beside a
case marks a shape that is pinned rather than endorsed; there are none today.

```
BURI_BLESS=1 cargo test -p buri --test formatting
```

A case named `textproto_*` is a build file rather than source — `buri format`
has two printers and this corpus pins both — and `every_checked_in_build_file_is_formatted`
holds the repository's own two hundred `BUILD.buri` and `REPO.buri` files to what
the second one prints.

It is deliberately outside the repository-wide walkers: an `input.buri` is
misformatted on purpose, and a suite asking whether every source in the
repository is already formatted would be asking these files a question they
exist to answer no to.

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

The `// EXPECT:` line says what the case is *about* in one phrase; the two
recorded files pin what a user actually reads — span, carets, notes, the order
of several diagnostics, every word of the prose. A reworded message is a change
to the product, so it should turn up as a diff and be looked at, rather than
pass silently because a substring happened to survive. After a deliberate
change:

```
BURI_BLESS=1 cargo test -p buri --test conformance rejected_programs
```

The JSON file is also where the four-part contract is enforced: **every
diagnostic must carry a `fix`**, and the harness fails the case if one does not.
That is a rule about the product, checked case by case rather than asserted in
prose.

Recording these found two bugs a substring check could never have. The
`did you mean` suggestion was picking an arbitrary winner among equally-close
candidates, so the same compiler on the same file printed `Add`, `Ord`, or `Eq`
depending on hash order. And the `= ...` lines sat one column right of the `|`
gutter above them, which nobody notices until the output is a file you diff.

**The repository corpus** exists because the two corpora above cannot hold a
build-system test. A reject case is synthesised as a single-package binary with
no dependencies, so nothing in it can express `missing-dep`, `dep-cycle`,
`visibility-violation`, or a tag conflict — which is most of what the build
system checks. A case there is a repository instead:

```
cli/tests/repositories/build-files/missing_dep/
  CASE.textproto      the manifest
  repo/               the repository, copied into a scratch tree and run in
  expected/lint.txt   what the CLI printed, recorded
```

The manifest is textproto — the format `REPO.buri` and `BUILD.buri` already
use, read by the toolchain's own parser. It is deliberately *not* named
`.buri`: only the tree under `repo/` is a Buri repository, and a file the
toolchain never reads should not wear the extension that says it does.

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
only way to ask what a command with no target operates on. `path` is for the
commands whose contract is about what they leave on disk rather than what they
print — `clean --outputs` and the `out/` symlink say almost nothing, and an
exit code cannot tell a cache that survived from one that was deleted and
rebuilt. Exactly one expectation per `path` step, spelled out: like `exit`, an
assertion here is never inferred.

`exit` is hand-written and required; only prose is blessed. Blessing can
rewrite what a diagnostic *says* and can never quietly turn a rejection into an
acceptance — a flipped exit code fails a `BURI_BLESS=1` run too.

Steps run in order against one scratch copy, so a case shows a rule firing and
then shows that the fix the diagnostic printed actually works. **Every case that
must stay clean ends with the edit that makes it fire**: a positive result is
only evidence when its negative twin sits next to it and cannot drift away.
`visibility_skips` is the shape to copy — a private library reached by its own
test suite and its co-located binary, clean, and then one edge from outside.

Goldens are path-stable without scrubbing, because every file enters the source
map under a repository-relative name. So `--> cmd/app/main.buri:9:6` in a
recorded file is also a path you can open.

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
in the manifest's own strings, and in reverse on the way back out, so that what
is compared against a golden and what `BURI_BLESS=1` records both hold the
placeholder. It is the same trick `<scratch>` already plays with the temporary
path, and it means blessing on either machine writes the same file.

What the pair guarantees is that the toolchain refuses it: `native_ready` ends
in `link.rs::can_link`, which is host-only, because the runtime archive the
binary embeds is built for the host. The two hosts' spellings are the same
width on purpose — a caret run is as wide as the line it underlines, and no
placeholder can stand in for one. Only the facts a case actually names are
substituted, so every other case's goldens come back byte for byte.

**The incrementality suite** reads `buri build --explain`, which prints one line
per action with its key and whether the cache served it. The claims in
HERMETICITY-AND-CACHING.md are about *which actions run*, and until that flag
existed nothing outside the toolchain could observe one. Keys are compared
between two states of one tree and never recorded — a key includes the
toolchain version, so a recorded one would move on every release.

**The golden transcript** catches a backend that produces a *different* answer
rather than no answer, on the one path no assertion inside a program can reach:
what a whole program prints on its way out. It is one line, so it lives as a
literal in the test rather than in a file — a change to what `//cmd/web` prints
should be read in the diff, not blessed. Everything else about rendering —
`${}` interpolation of every type, float formatting, captured stdout — is
asserted from inside the conformance suite, where a wrong answer is a failed
`assert.eq` rather than a diff.

**The emitted-JavaScript corpus** is the only suite that looks at the output
rather than the answer. An optimisation is invisible to every other suite here
by construction: removing an allocation, a call frame or a redundant test
changes no value anywhere, so without a record of what the backend emits, a
pass lands unseen and regresses unnoticed. Each case is one small program
exercising one construct:

```
cli/tests/golden_javascript/enum_match/
  main.buri       the program
  expected.mjs    the generated code, with the runtime removed
  expected.out    what it prints
cli/tests/golden_javascript/sizes.txt   release artifact sizes, whole corpus, one file
```

The runtime is removed because it is the same thousand lines in every case and
is not what any pass changes; what is left is exactly what the backend
produced, with debug names, so the diff is readable. `sizes.txt` is one file
rather than one number per case so that the size effect of a change is a single
diff, and each case must be smaller in release than in debug.

**`expected.out` is recorded once and never re-recorded.** `expected.mjs` is a
record of *how* a program compiles and is meant to move with every pass;
`expected.out` is a claim about what it *computes*, and no pass may move it. So
`BURI_BLESS=1` writes it when it is missing and refuses to overwrite it
afterwards — to change one deliberately, delete it and re-record. Every case is
also run in both build modes and the two must print the same bytes.

This is not hypothetical. A parallel-move optimisation in the tail-call
rebinding read a parameter after overwriting it, turning a sum of `5050` into
`4950`; the conformance suite has no tail-recursive accumulator that shape, so
this corpus was the only thing that saw it — and had `expected.out` been
blessable, blessing would have recorded the wrong answer and moved on.

```
BURI_BLESS=1 cargo test -p buri --test golden_javascript
```

Blessing without reading the diff is the one way this suite proves nothing.

## Properties pinned outside the corpora

`conformance.rs` also holds the checks that are about the toolchain rather than
the language:

- **Tail calls run in constant stack on V8.** Ten million bounces through a
  self-recursive function and a mutually recursive pair, on an engine with no
  native proper tail calls. This is the test that says the compiler does the
  elimination itself rather than leaning on JavaScriptCore.
- **`--release` and `--debug` agree.** Mangling, dead code elimination,
  constant folding and runtime tree-shaking may not change what a program
  computes or prints. The whole conformance suite is run both ways and has to
  pass the same number of assertions, and the monorepo's binary has to print
  the same bytes — while the release artifact has to be *smaller*, so identical
  behaviour cannot be bought by doing nothing.
- **Builds are reproducible.** The worked monorepo, copied to two different
  directories and built in each, produces byte-identical output — so neither a
  path nor a hash-map ordering leaks in.
- **The cache cannot serve a stale answer.** Build, edit, rebuild, and check
  the program's *behaviour* changed — then revert and check the original entry
  comes back.
- **Exit codes distinguish bad code from a bad invocation**, which is the
  distinction CLI.md draws between 1 and 2.
- **`format --check` reports without rewriting**, and formatting is a fixed
  point.

`backend_agreement.rs` is the one outside `conformance.rs`, and it is about the
*pair* of backends rather than about either: one `.buri` source compiled through
`actions::prepare` and `backend::select` twice — JavaScript under `bun`, native
through Cranelift or LLVM and `cc` — with the two outputs compared byte for byte.
Every row of `design/native/VALUE-MODEL.md` §12 is a `#[test]`, so a failure
names the row, and `every_row_of_the_table_names_a_test_that_exists` reads the
table back and fails on a row whose test is missing. A row the native surface
cannot reach yet gets a gap test naming the missing intrinsic *and* an
`#[ignore]`d agreement test beside it, so neither can rot alone. It skips with a
printed reason where `native_ready` is false or no JavaScript engine is on the
path, and compiles to nothing with `--no-default-features`.
