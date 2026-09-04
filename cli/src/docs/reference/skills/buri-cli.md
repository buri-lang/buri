---
name: buri-cli
description: Use when running the buri toolchain — build, run, test, lint, format, gen, query, docs, watch — or when a diagnostic prints a [code] you want explained.
---

# Buri: the CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, serves its own documentation, and hosts the
language server. There is no package manager, no task runner, and no
configuration of the CLI beyond `REPO.buri`.

`buri --help` prints the table below; `buri docs cli <command>` is the page for
one command, with a flag table generated from the same table that dispatches —
so a page cannot describe a flag the binary does not accept.

## Commands

| Command | What it does |
|---|---|
| `buri init [directory]` | write a new repository: a library, a binary, a test, and these skills |
| `buri build [targets]` | compile |
| `buri test [targets]` | compile and run test suites |
| `buri run <target> [-- args]` | build one binary and execute it |
| `buri format [paths]` | format `.buri` sources and build files |
| `buri lint [targets]` | static checks beyond type checking |
| `buri gen [targets]` | regenerate the fields of a `BUILD.buri` that restate the sources |
| `buri query <expr>` | ask about the build graph |
| `buri docs [topic]` | the language, the build system, and this CLI |
| `buri add skills [directory]` | write these agent skills into `.agent/skills` |
| `buri lsp` | language server, over stdio |
| `buri clean` | drop the local cache |
| `buri version` | toolchain version; `--verbose` adds the executable's hash |

Target arguments accept labels and patterns — `//lib/money`, `//lib/...`,
`//...`. **With no argument, a command operates on the whole repository** — bare
is `//...`, whatever directory you are standing in. All commands are safe to run
concurrently; a file lock serializes cache writes.

## Exit codes

| | |
|---|---|
| `0` | success; for `test`, every test passed |
| `1` | the thing you asked *about* is wrong — a build, lint, or test failure |
| `2` | the thing you asked *with* is wrong — a malformed invocation or an unparseable build file |

So `buri test //...` and `buri format --check` are usable directly as gates.

## Flags

Three are global: `--verbose`, `--color[=never]` (`NO_COLOR` is honoured too),
and `--error-format=json`. Everything else belongs to the commands that read
it, and naming one elsewhere is an error that says which commands do take it.

| Flag | Commands | Meaning |
|---|---|---|
| `--release` / `--debug` | build, test, run | optimize and minify, or the readable default. Exclusive. |
| `--output=<selector>` | build, test, run | which output to build, or which a suite runs on — `--output=js`, `--output=linux/x86_64` |
| `--force` | build, test, run | ignore the cache and run the action |
| `--explain` | build, test, run | one line per action: whether it ran or the cache served it, and the key |
| `--check-reproducible` | build | build twice in separate directories and compare byte for byte |
| `--filter=<substring>` | test | run only the tests whose name contains this |
| `--watch` | test | re-run on every change to a declared input, until interrupted |
| `--check` | format, gen, docs | report what would change and exit 1, writing nothing |
| `--fix` | lint | apply the findings that have one mechanical answer |
| `--format=<human\|markdown\|json>` | docs | how a page is printed |
| `--dense` | build, test, run, lint, docs | fewer tokens: headings and examples only, and on a build, diagnostics without the explanation under them |
| `--outputs` | clean | remove build outputs but keep the action cache |
| `--self-check` | version | type-check the embedded standard library against itself |

Everything after a bare `--` goes to the program `buri run` executes.

## Diagnostics, and the `[code]`

Every diagnostic answers four questions in a fixed order — **where** (a span
with a caret), **expected**, **actual**, and **fix**. `expected` and `actual`
are omitted where the error is not a mismatch; `fix` never is.

```
error: expected `I32`, found `I64` [type-mismatch]
 --> cmd/report/main.buri:6:7
  |
6 |   a + b
  |       ^ the left operand's type is `I32`
  |
  = expected: `I32`
  = actual: `I64`
  = there is no implicit promotion of any kind
  = fix: convert explicitly with `.toI32()?`, which returns a `Result<I32, RangeError>`
```

**The bracketed name at the end of the message is a code you can look up.**

```
buri docs error type-mismatch    one diagnostic in full, with a program that provokes it
buri docs error                  every compiler code, listed
buri docs lint missing-dep       the same for a `buri lint` finding
buri docs lint                   every lint code, listed
```

Compiler and build diagnostics are in the `error` catalogue, `buri lint`
findings in the `lint` one; both print their code the same way.

Each error page carries a program that provokes the error, and the test suite
checks that it still does — so a page cannot describe an error the compiler has
stopped emitting. The first time a code appears in a run, the explanation is
printed under the diagnostic; later occurrences print the short form only, and
`--dense` suppresses it entirely.

### `--error-format=json`

For editors, CI, and coding agents. One JSON object per diagnostic, one per
line, on stderr. It implies `--color=never`.

```
buri build //... --error-format=json
```

| Field | |
|---|---|
| `severity` | `error`, `warning`, or `note` |
| `message` | the one-line summary |
| `code` | the diagnostic's code |
| `location` | `file`, `line`, `column`, `endLine`, `endColumn`, the source `text` of that line, and an optional `label`; `null` when the diagnostic is about the invocation |
| `expected`, `actual` | present on a mismatch |
| `notes` | background, in order |
| `fix` | the edit to make. Always present |
| `related` | other locations, each shaped like `location` |

Lines are independent, so a consumer can stream them. An absent field means
"not applicable" rather than "empty".

## The commands, in the order you reach for them

### `build`

A binary produces an artifact under `.buri/out/<platform>/<package>/`; a
library is type-checked, because a library has no artifact of its own — so
`buri build //lib/money` means "tell me whether this library is correct".

### `test`

Builds the targets with their `test.sources`, runs every `test` declaration,
and reports one line per failure and a summary. See the `buri-testing` skill.

`--watch` re-runs the same invocation whenever a declared input moves: the
closure's entry points, `sources`, `proto_sources` and `testing/` sources; the
suite's `sources`; every `BUILD.buri`; and `REPO.buri`. Polled with
one `stat` each every 150 ms, so a burst of writes is one run. A run that had
nothing to do prints nothing at all. **A new file is not watched until
something declares it** — run `buri gen`, and the loop sees the build file
change. A `BUILD.buri` that stops parsing prints its diagnostics and the loop
keeps watching. `--watch` is refused with `--force` and when stdout is not a
terminal.

### `run`

Builds exactly one binary and executes it. This is the one command that
produces a process with real authority — the real filesystem, the real
environment — and what the program can actually do is still bounded by the
context its `main` builds.

### `lint`

The static checks that are not type errors: sources declared but absent, a
source no rule names, a dependency declared and unused, one used and
undeclared, visibility and tag violations, package and import cycles, and the
hygiene rules — an import nothing uses, an `export` nothing reaches, a test
that asserts nothing. Each finding carries a stable code.

`--fix` applies the findings with exactly one mechanical answer, then runs the
whole check again from the files on disk. Build-file findings (`missing-dep`,
`unused-library`, `duplicate-source`) are handed to `buri gen`;
`unused-import` is applied as bytes. `--fix` edits and does **not** reformat.
Where two edits in one file overlap, none of that file's are applied.

Exits 1 if it reported anything at all: every finding is a warning, and severity
does not gate the exit code, because running the linter is already the request
to be told.

One catalogue, one severity — warning — the same in every repository, and no
per-file suppression comment. `REPO.buri`'s `lint` block is where a repository
decides the rest, for the whole repository at once: `check_during_build` runs
the catalogue during `buri build` and `buri test` too, `fail_on_finding` makes a
finding fail whichever command reported it, and `rules { default: ENABLED|
DISABLED, <lint_code>: bool }` says which rules run —
`enabled(rule) = override.unwrap_or(default)`, one field per lint code with the
hyphens underscored, an unknown name refused as `unknown-field`. Both booleans
default to false, every rule defaults to on, and a report from a repository that
turned rules off says which ones.

### `format`

One canonical layout, no options: four-space indent, one field per line in
build files, and the **leading** run of imports sorted (`core/*` before `//*`,
then by path). Formatting is a fixed point. `--check` writes nothing and exits
1 if anything would change — the form for CI. Import order is therefore not a
lint: an unsorted run is a file that has not been formatted.

### `gen`

Rewrites the seven fields that restate the sources of build files **that
already exist**, sorted, and touches nothing else. It never creates a build
file. In a package with both rules, a file no rule lists goes to the rule whose
entry point reaches it; a file reached from both or neither is an error. With no
target argument it regenerates every package — bare `buri gen` is
`buri gen //...`, the default `buri format` already has.

### `query`

Answers questions about the build graph without building anything.

```
buri query 'deps(//cmd/server)'             what it depends on, transitively
buri query 'rdeps(//lib/money)'             what depends on it
buri query 'path(//cmd/web, //lib/store)'   the edge chain, with the declaring line
buri query 'tags(//cmd/server)'             every tag in its closure, and who contributed it
buri query 'platforms(//cmd/web)'           the platforms its closure permits
buri query 'sources(//lib/money)'           the files the rule names
```

`path` is the one that earns its place: the answer to "why does the browser
build pull in the database layer" is an edge.

### `docs`

Served from the binary, so it works outside a repository and cannot go stale —
every fenced example in every page is compiled by the test suite, and the ones
that print something are executed and compared.

```
buri docs                          every page, grouped
buri docs language/effects         one topic
buri docs cli build                one command, flags generated from the dispatch table
buri docs error result-discarded   one diagnostic, with a program that provokes it
buri docs core/list                a standard library module, rendered from its source
buri docs core/list.map            one item of one module
buri docs lint missing-dep         one lint finding in full
buri docs search compare ints      every page at once, by name or by intent
buri docs manifest                 every id and output shape, for an agent
```

Search takes words rather than a name: it reads the prose inside every page and
a table of concepts, so "compare ints" reaches `core/order` and "fixture"
reaches `core/host/testing`. Each hit is printed as the command that reads it.

For an agent: `--format=json` prints one object on one line, and `--dense`
drops prose but keeps every heading and **every example**.

### `clean`

Removes `.buri/out`, the action cache under `.buri/cache`, the staged objects
under `.buri/link/`, and the `out` symlink. `--outputs` drops `.buri/out`
alone. Reaching for it to fix a build is worth reporting as a bug: the cache is
keyed on the content of every input, so a stale entry is a defect.

### `init`

Writes a working repository into an empty directory — `REPO.buri`, a library,
a binary that depends on it, a test suite, a `.gitignore`, and these skills —
and creates the directory if it is not there. What it writes builds, tests,
lints and formats clean immediately. It never writes over a file: a `REPO.buri`
already at the target, or any other collision, stops it with exit 2 before the
first byte. The one exception is an existing `.gitignore` — git owns that name,
so the build's entries are appended below its lines instead.

### `add skills`

Writes the toolchain's agent skills into `.agent/skills/<name>/SKILL.md` under
the working directory, or under a directory you name. Re-running refreshes
every `buri-*` skill and leaves every other skill alone, so an upgraded
compiler updates them in place.

## A first session in an unfamiliar repository

```
buri --help                    the command table
buri docs                      every page the binary ships
buri query 'deps(//...)'       what is here
buri build //...               does it compile
buri test //...                does it pass
buri lint //...                does it obey the graph rules
buri format --check            is it formatted
```
