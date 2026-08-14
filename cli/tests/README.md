# Testing the toolchain

A compiler that exits 0 has proved nothing. These suites are arranged so that a
wrong *answer* fails, not merely a program that fails to run.

| Suite | Where | What it proves |
|---|---|---|
| Unit tests | `cli/src/**` (`#[cfg(test)]`) | The lexer, parser, textproto reader, type unifier, JS printer, minifier, SHA-256, and SCC finder do what they claim in isolation. |
| Corpus | `cli/tests/corpus.rs` | Every `.buri` file in the repository parses; every build file reads; formatting is a fixed point. |
| Standard library | `cli/tests/stdlib.rs` | `core/*` typechecks against itself — the first program the compiler ever sees. |
| Conformance | `cli/tests/conformance/` | A Buri repository whose `test/` directories assert on language semantics, run through the real `buri test`. |
| Rejection | `cli/tests/reject/` | Programs that must **not** compile, each annotated with the diagnostic it must produce. |
| Crash | `cli/tests/crash/` | Programs that must compile, then crash, saying why. Overflow, division by zero, `crash`. |
| Golden | `cli/tests/golden/` | The exact stdout of all 22 example programs. |
| Mutation | `cli/tests/mutants.sh` | That the suites above are *capable of failing*. |

Everything but the unit tests drives the real `buri` binary, because that is
what a user runs.

## Running them

```
cargo test -p buri                       # everything except mutation testing
cargo test -p buri --test conformance    # the end-to-end suites
cli/tests/mutants.sh                     # inject bugs, check they are caught
```

## Why each shape exists

**The conformance repository** is ordinary Buri. Its packages are libraries
whose `test/` directories hold `test "..." { ... }` declarations, so the thing
being exercised is the whole pipeline — parse, check, monomorphize, emit,
minify, run — and the assertion is on a value, not on an exit code.

`lib/canary` exists to keep the suite honest: `conformance_suite_can_fail`
rewrites a constant in it and asserts the runner notices. A suite that cannot
fail is not evidence.

**Rejection and crash corpora** cover what assertions cannot. A program that
must not compile has no runtime to assert in, and a crash cannot be caught —
there is no `catch` in the language — so both get a harness that compiles or
runs the program and checks the diagnostic.

Each file carries its expectation on its first line:

```buri
// EXPECT: may not be discarded      (tests/reject)
// CRASH: integer overflow           (tests/crash)
```

**Golden transcripts** are what catch a backend that produces a *different*
answer rather than no answer. They were written by reading each example and
working out what it should print, not by capturing whatever the compiler
happened to emit. A line that genuinely varies between runs — a clock reading,
a roll of the real `Rand`, a network result — is declared volatile and compared
only up to its prefix, so the line still has to be present and still has to say
what it is:

```
# ARGS: words.txt --top 4
# VOLATILE: dice        =
```

Fixture files a program reads live in `tests/golden/<name>.files/`.

**Mutation testing** is the part that makes the rest mean something. Each
mutant is a realistic bug — division that stops truncating toward zero,
comparison that returns the wrong `Order`, tail-call elimination turned off,
the must-use rule turned off — injected into the compiler or its runtime. The
suite is then run. A mutant that *survives* is a hole, and the script exits
non-zero and names it.

## Properties pinned outside the corpora

`conformance.rs` also holds the checks that are about the toolchain rather than
the language:

- **Tail calls run in constant stack on V8.** Ten million bounces through a
  self-recursive function and a mutually recursive pair, on an engine with no
  native proper tail calls. This is the test that says the compiler does the
  elimination itself rather than leaning on JavaScriptCore.
- **`--release` and `--debug` agree.** Mangling, dead code elimination,
  constant folding and runtime tree-shaking may not change what a program
  prints. Run over the whole example corpus, both ways, diffed.
- **Builds are reproducible.** The same source built in two different
  directories produces byte-identical output, so no path leaks in.
- **The cache cannot serve a stale answer.** Build, edit, rebuild, and check
  the program's *behaviour* changed — then revert and check the original entry
  comes back.
- **Exit codes distinguish bad code from a bad invocation**, which is the
  distinction CLI.md draws between 1 and 2.
- **`format --check` reports without rewriting**, and formatting is a fixed
  point.
