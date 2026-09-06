---
name: buri-cli
description: Use when running the buri toolchain — build, run, test, lint, format, gen, query, docs, watch — or when a diagnostic prints a [code] you want explained.
---

# Buri: the CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, serves its own documentation, and hosts the
language server. There is no package manager, no task runner, and no
configuration of the CLI beyond `REPO.buri`.

`buri --help` prints the table below. `buri docs cli <command>` is the page for
one command, and its flag table comes from the same table that dispatches, so a
page cannot describe a flag the binary does not accept.

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

Target arguments accept labels and patterns: `//lib/money`, `//lib/...`,
`//...`. **With no argument, a command works on the whole repository** — bare
means `//...`, whatever directory you are standing in. Every command is safe to
run concurrently, since a file lock serializes cache writes.

## Exit codes

| | |
|---|---|
| `0` | success; for `test`, every test passed |
| `1` | the thing you asked *about* is wrong — a build, lint, or test failure |
| `2` | the thing you asked *with* is wrong — a malformed invocation or an unparseable build file |

So you can use `buri test //...` and `buri format --check` directly as gates.

## Flags

Three are global: `--verbose`, `--color[=never]` (`buri` honours `NO_COLOR`
too), and `--error-format=json`. Every other flag belongs to the commands that
read it, and naming one elsewhere is an error that says which commands do.

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

Every diagnostic answers four questions in a fixed order: **where** (a span
with a caret), **expected**, **actual** and **fix**. An error that is not a
mismatch drops `expected` and `actual`. Nothing ever drops `fix`.

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

The `error` catalogue holds compiler and build diagnostics, and the `lint` one
holds `buri lint` findings. Both print their code the same way.

Each error page carries a program that provokes the error, and the test suite
checks that it still does, so a page cannot describe an error the compiler has
stopped emitting. The first time a code appears in a run, `buri` prints the
explanation under the diagnostic. Later occurrences print the short form only,
and `--dense` drops it entirely.

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

A binary produces an artifact under `.buri/out/<platform>/<package>/`. A
library has no artifact of its own, so `buri build //lib/money` type-checks it:
it means "tell me whether this library is correct".

### `test`

It builds the targets with their `test.sources`, runs every `test` declaration,
and reports one line per failure plus a summary. See the `buri-testing` skill.

`--watch` re-runs the same invocation whenever a declared input moves: the
closure's entry points, `sources`, `proto_sources` and `testing/` sources; the
suite's `sources`; every `BUILD.buri`; and `REPO.buri`. It polls each with one
`stat` every 150 ms, so a burst of writes is one run. A run with nothing to do
prints nothing at all. **Nothing watches a new file until something declares
it** — run `buri gen`, and the loop sees the build file change. A `BUILD.buri`
that stops parsing prints its diagnostics, and the loop keeps watching. `buri`
refuses `--watch` with `--force`, and when stdout is not a terminal.

### `run`

It builds exactly one binary and executes it. This is the one command that
produces a process with real authority: the real filesystem, the real
environment. The context its `main` builds still bounds what the program can
do.

### `lint`

The static checks that are not type errors: sources declared but absent, a
source no rule names, a dependency declared and unused, one used and
undeclared, visibility and tag violations, package and import cycles, and the
hygiene rules — an import nothing uses, an `export` nothing reaches, a test
that asserts nothing. Each finding carries a stable code.

It reads `sources`, `test { sources }` and `testing { sources }` alike, so a
suite meets the same rules and the report carries the checker's errors about all
three. Only `dead-code` and `ctx-rebinding` never fire in a test source, and a
`testing/` module's surface is `testing/lib.buri`.

`--fix` applies the findings with exactly one mechanical answer, then runs the
whole check again from the files on disk. It hands the build-file findings
(`missing-dep`, `unused-library`, `duplicate-source`) to `buri gen`, and applies
`unused-import` as bytes. `--fix` edits and does **not** reformat. Where two
edits in one file overlap, it applies none of that file's.

It exits 1 if it reported anything at all. Every finding is a warning, and
severity does not gate the exit code, because running the linter is already
asking to be told.

One catalogue, one severity — warning — the same in every repository, and no
per-file suppression comment. `REPO.buri`'s `lint` block decides the rest, for
the whole repository at once. `check_during_build` runs the catalogue during
`buri build` and `buri test` too. `fail_on_finding` makes a finding fail
whichever command reported it. `rules { default: ENABLED|DISABLED, <lint_code>:
bool }` says which rules run — `enabled(rule) = override.unwrap_or(default)`,
one field per lint code with the hyphens underscored, and an unknown name
refused as `unknown-field`. Both booleans default to false, every rule defaults
to on, and a report from a repository that turned rules off says which ones.

### `format`

One canonical layout, no options: four-space indent, one field per line in
build files, and the **leading** run of imports sorted (`core/*` before `//*`,
then by path). Formatting is a fixed point. `--check` writes nothing and exits 1
if anything would change, which is the form for CI. So import order is not a
lint: an unsorted run is a file nobody formatted.

### `gen`

It rewrites the seven fields that restate the sources, in build files **that
already exist**, sorted, and touches nothing else. It never creates a build
file. In a package with both rules, a file no rule lists goes to the rule whose
entry point reaches it, and a file reached from both or neither is an error.
With no target argument it regenerates every package: bare `buri gen` is
`buri gen //...`, the default `buri format` already has.

### `query`

It answers questions about the build graph without building anything.

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

The binary serves these pages, so they work outside a repository and cannot go
stale. The test suite compiles every fenced example in every page, then runs the
ones that print something and compares the output.

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

Search takes words rather than a name. It reads the prose inside every page and
a table of concepts, so "compare ints" reaches `core/order` and "fixture"
reaches `core/host/testing`. Each hit prints as the command that reads it.

**Explore before you hand-roll.** Bare `buri docs` is the whole index — every
topic, every command, every diagnostic code and every standard library module —
and it is the cheapest call here. When you are about to write a comparator, a
table of hex digits, a `groupBy`, a base64 encoder or a date calculation, read
the module first: `buri docs core/order`, `core/bytes`, `core/map`, `core/list`,
`core/date`. Search by intent when you cannot name the module. A module's page
comes from the source the compiler checked, so it cannot describe a function
that does not exist, and anything already in `core/*` is code you do not have to
write, test, or get right.

For an agent: `--format=json` prints one object on one line, and `--dense`
drops prose but keeps every heading and **every example**.

### `clean`

It removes `.buri/out`, the action cache under `.buri/cache`, the staged
objects under `.buri/link/`, and the `out` symlink. `--outputs` drops
`.buri/out` alone. If you reach for it to fix a build, report that as a bug: the
cache key is the content of every input, so a stale entry is a defect.

### `init`

It writes a working repository into an empty directory — `REPO.buri`, a
library, a binary that depends on it, a test suite, a `.gitignore`, and these
skills — and creates the directory if it is not there. What it writes builds,
tests, lints and formats clean immediately. It never writes over a file: a
`REPO.buri` already at the target, or any other collision, stops it with exit 2
before the first byte. An existing `.gitignore` is the one exception, since git
owns that name, so `init` appends the build's entries below its lines.

### `add skills`

It writes the toolchain's agent skills into `.agent/skills/<name>/SKILL.md`,
under the working directory or under a directory you name. Re-running refreshes
every `buri-*` skill and leaves every other skill alone, so an upgraded compiler
updates them in place.

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
