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
| Golden | `WEB_STDOUT` in `conformance.rs` | The exact stdout of the worked monorepo's JS binary. |

Everything but the unit tests drives the real `buri` binary, because that is
what a user runs.

## Running them

```
cargo test -p buri                       # everything
cargo test -p buri --test conformance    # the end-to-end suites
```

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
