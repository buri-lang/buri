# Testing the toolchain

A compiler that exits 0 has proved nothing. These suites are arranged so that a
wrong *answer* fails, not merely a program that fails to run.

| Suite | Where | What it proves |
|---|---|---|
| Unit tests | `cli/src/**` (`#[cfg(test)]`) | The lexer, parser, textproto reader, type unifier, JS printer, minifier, SHA-256, and SCC finder do what they claim in isolation. |
| Corpus | `cli/tests/corpus.rs` | Every `.buri` file in the repository that is meant to compile parses; every build file reads; formatting is a fixed point. |
| Standard library | `cli/tests/stdlib.rs` | `core/*` typechecks against itself — the first program the compiler ever sees. |
| Conformance | `cli/tests/conformance/` | A Buri repository whose `test/` directories assert on language semantics, run through the real `buri test`. |
| Rejection | `cli/tests/reject/` | Programs that must **not** compile, one directory each, holding the diagnostics exactly as the terminal and `--error-format=json` print them. |
| Abort | `cli/tests/crash/` | Programs that must compile, then abort, saying why. Division by zero, a shift past the width of its type, an empty random range. |
| Repositories | `cli/tests/repos/` | Whole repositories, one per build-system rule, each with a manifest of what the CLI does in it and the output that produces. |
| Incrementality | `cli/tests/incrementality.rs` | What the cache may and may not do, read off the `--explain` transcript. |
| Golden | `WEB_STDOUT` in `conformance.rs` | The exact stdout of the worked monorepo's JS binary. |

Everything but the unit tests drives the real `buri` binary, because that is
what a user runs.

## Running them

```
cargo test -p buri                       # everything
cargo test -p buri --test conformance    # the language suites
cargo test -p buri --test repos          # the build-system suites
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
cli/tests/repos/build-files/missing_dep/
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
edit { file: "cmd/app/BUILD.buri"  replace: "..."  with: "..." }
file { path: "cmd/f/main.buri"  golden: "formatted.buri" }
```

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
