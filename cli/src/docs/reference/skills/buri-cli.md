---
name: buri-cli
description: Use when running the buri toolchain — build, run, test, lint, format, gen, query, docs, watch — or when a diagnostic prints a [code] you want explained.
---

# Buri: the CLI

One binary does everything. There is no package manager, no task runner, and no
configuration beyond `REPO.buri`. `buri docs cli <command>` is the page for one
command.

## Commands

| Command | What it does |
|---|---|
| `buri init [directory]` | write a new repository: a library, a binary, a test, and these skills |
| `buri build [targets]` | compile |
| `buri test [targets]` | compile and run test suites |
| `buri run <target> [-- args]` | build one binary and execute it |
| `buri format [paths or targets]` | format `.buri` sources and build files |
| `buri lint [targets]` | static checks beyond type checking |
| `buri gen [targets]` | regenerate the fields of a `BUILD.buri` that restate the sources |
| `buri query <expr>` | ask about the build graph |
| `buri docs [topic]` | the language, the build system, and this CLI |
| `buri add skills [directory]` | write these agent skills into `.agent/skills` |
| `buri lsp` | language server, over stdio |
| `buri clean` | drop the local cache |
| `buri version` | toolchain version; `--verbose` adds the executable's hash |

Targets are labels or patterns: `//lib/money`, `//lib/...`, `//...`. **With no
argument, a command works on the whole repository** (`//...`), whatever
directory you're in. `buri format` also takes a file or directory path. Every
command is safe to run concurrently.

## Exit codes

| | |
|---|---|
| `0` | success; for `test`, every test passed |
| `1` | the thing you asked *about* is wrong — a build, lint, or test failure |
| `2` | the thing you asked *with* is wrong — a malformed invocation or an unparseable build file |

So `buri test //...` and `buri format --check` work directly as gates.

## Flags

Three are global: `--verbose`, `--color[=never]` (`NO_COLOR` works too), and
`--error-format=json`. Every other flag belongs to specific commands, and
passing it elsewhere is an error that names them.

| Flag | Commands | Meaning |
|---|---|---|
| `--release` / `--debug` | build, test, run | optimize and minify, or the readable default. Exclusive. |
| `--output=<selector>` | build, test, run | which output to build — `--output=node`, `--output=native/linux-x86_64` — or, on `test`, which backend a suite runs on: `js` or `native` |
| `--force` | build, test, run | ignore the cache and run the action |
| `--explain` | build, test, run | one line per action: whether it ran or the cache served it, and the key |
| `--check-reproducible` | build | build twice in separate directories and compare byte for byte |
| `--filter=<substring>` | test | run only the tests whose name contains this |
| `--watch` | test, run | re-run on every change to a declared input, until interrupted; on `buri run` it rebuilds the page being served |
| `--port=<port>` | run | where the page `buri run` serves listens — default 4000, `0` takes whatever is free |
| `--check` | format, gen, docs | report what would change and exit 1, writing nothing |
| `--fix` | lint | apply the findings that have one mechanical answer |
| `--format=<human\|markdown\|json>` | docs | how a page is printed |
| `--dense` | build, test, run, lint, docs | fewer tokens: headings and examples only, and on a build, diagnostics without the explanation under them |
| `--outputs` | clean | remove build outputs but keep the action cache |
| `--self-check` | version | type-check the embedded standard library against itself |

Everything after a bare `--` goes to the program `buri run` executes.

## Diagnostics, and the `[code]`

Every diagnostic gives **where**, **expected**, **actual** and **fix**, in that
order. Non-mismatch errors drop `expected` and `actual`. Nothing drops `fix`.

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

**The bracketed name is a code you can look up.**

```
buri docs error type-mismatch    one diagnostic in full, with a program that provokes it
buri docs error                  every compiler and build code, listed
buri docs lint missing-dep       the same for a `buri lint` finding
buri docs lint                   every lint code, listed
```

The first time a code appears in a run, `buri` prints its explanation under the
diagnostic. Later occurrences print the short form, and `--dense` drops it.

### `--error-format=json`

One JSON object per diagnostic, one per line, on stderr. Implies
`--color=never`.

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

An absent field means "not applicable", not "empty".

## The commands

### `build`

A binary's artifact lands under `.buri/out/<platform>/<package>/`. A library
has no artifact, so `buri build //lib/money` just type-checks it.

### `test`

Builds the targets with their `test.sources`, runs every `test` declaration,
and prints one line per failure plus a summary. See the `buri-testing` skill.

`--watch` re-runs whenever a declared input changes: the closure's entry points,
`sources`, generator `inputs` and `testing/` sources; the suite's `sources`;
every `BUILD.buri`; and `REPO.buri`. It polls every 150 ms, so a burst of writes
is one run, and a run with nothing to do prints nothing. **A new file isn't
watched until something declares it** — run `buri gen`. A `BUILD.buri` that
stops parsing prints its diagnostics and the loop keeps going. `--watch` is
refused with `--force`, and for `test`, when stdout isn't a terminal.

### `run`

Builds exactly one binary and executes it with real authority: the real
filesystem and environment. The context its `main` builds still bounds what the
program can do.

A `web` output is served instead. The address prints once, files under the
artifact directory are served as themselves, and every other path gets the
entry shell so the page's router sees the typed address. Nothing is cached,
`--watch` rebuilds into the next request, and a website's worker half isn't run.

### `lint`

Checks that aren't type errors: sources declared but absent, a source no rule
names, a dependency unused or undeclared, visibility and tag violations,
package and import cycles, and hygiene rules — an unused import, an
unreachable `export`, a test that asserts nothing. Each finding has a stable
code.

It reads `sources`, `test { sources }` and `testing { sources }` alike, and
reports the checker's errors for all three. Only `dead-code` and
`ctx-rebinding` never fire in a test source. A `testing/` module's surface is
`testing/lib.buri`.

`--fix` applies the findings with exactly one mechanical answer, then re-checks
from disk. It hands `missing-dep`, `unused-library` and `duplicate-source` to
`buri gen`, and applies `unused-import` as bytes. It does **not** reformat. If
two edits in one file overlap, it applies none of that file's.

It exits 1 if it reported anything. Every finding is a warning, the catalogue
is the same in every repository, and there's no per-file suppression comment.
`REPO.buri`'s `lint` block picks where it runs and which rules run; see the
`buri-build` skill.

### `format`

One canonical layout, no options: four-space indent, one field per line in
build files, and the **leading** run of imports sorted (`core/*` before `//*`,
then by path). Use `--check` in CI.

### `gen`

Rewrites the seven fields that restate the sources, sorted, in build files
**that already exist**. It touches nothing else and never creates a build file.
In a package with both rules, an unlisted file goes to the rule whose entry
point reaches it; a file reached from both or neither is an error.

### `query`

Answers questions about the build graph without building.

```
buri query 'deps(//cmd/server)'             what it depends on, transitively
buri query 'rdeps(//lib/money)'             what depends on it
buri query 'path(//cmd/web, //lib/store)'   the edge chain, with the declaring line
buri query 'tags(//cmd/server)'             every tag in its closure, and who contributed it
buri query 'platforms(//cmd/web)'           the platforms its closure permits
buri query 'sources(//lib/money)'           the files the rule names
```

### `docs`

The binary serves these pages, so they work outside a repository and can't go
stale.

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

Search takes intent: "compare ints" reaches `core/order`, "fixture" reaches
`platform/effect/testing`. Each hit prints as the command that reads it.

**Explore before you hand-roll.** Bare `buri docs` is the cheapest call here.
Before writing a comparator, a hex-digit table, a `groupBy`, a base64 encoder or
a date calculation, read `buri docs core/order`, `core/bytes`, `core/map`,
`core/list`, `core/date`.

`--format=json` prints one object on one line. `--dense` drops prose but keeps
every heading and **every example**.

### `clean`

Removes `.buri/out`, the action cache under `.buri/cache`, staged objects under
`.buri/link/`, and the `out` symlink. `--outputs` drops `.buri/out` alone. If
you need it to fix a build, report that as a bug.

### `init`

Writes a working repository — `REPO.buri`, a library, a binary that depends on
it, a test suite, a `.gitignore`, and these skills — creating the directory if
needed. It never overwrites a file: any collision stops it with exit 2 before
writing anything. The exception is an existing `.gitignore`, which gets the
build's entries appended.

### `add skills`

Writes the agent skills into `.agent/skills/<name>/SKILL.md`, under the working
directory or one you name. Re-running refreshes every `buri-*` skill and leaves
others alone.

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
