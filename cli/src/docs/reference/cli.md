# The CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, serves this documentation, and hosts the
language server. There is no second tool to install, no package manager, no task
runner, and no configuration of the CLI itself beyond
[`repo-config.md`](./build/repo-config.md).

Each command's synopsis and flag table comes from the table that dispatches it,
so neither can describe a flag the binary rejects or omit one it accepts. This
page covers what every command shares: how you name a target, what the two
global flags do, what the exit codes mean, and the shape of a diagnostic.

## Naming targets

Target arguments take labels and patterns: `//lib/money`, `//cmd/server`,
`//lib/...`, `//...`. A label names a package and every target in it. Leave the
argument off and the command covers the whole repository. Bare means `//...`,
and the directory you happen to be standing in never changes what a command
means. Run as many commands at once as you like; a file lock serializes cache
writes.

## The two global flags

`--color=never` drops the ANSI escapes. `--error-format=json` prints diagnostics
as one JSON object per line, and implies `--color=never`. Every other flag
belongs to a single command, and that command's page lists it.

## Exit codes

`0` success · `1` the thing you asked about is wrong · `2` the thing you asked
*with* is wrong.

A lint finding exits `1`. So does a compile error, a failing test, and a syntax
error in a source file: each one answers the question you asked. `2` is the run
that never started — a target pattern that names nothing, a flag that does not
exist, a build file that does not parse.

## Diagnostics

Every diagnostic answers four questions in the same order, so a person and a
program both read it the same way:

```
error: expected `I32`, found `I64`
 --> cmd/report/main.buri:6:7
  |
6 |   a + b
  |       ^ the left operand's type is `I32`
  |
  = expected: `I32`
  = actual: `I64`
  = there is no implicit promotion of any kind
  = fix: convert explicitly with `.toI32()?`, which returns a `Result<I32, RangeError>` because not every `I64` fits
```

| | |
|---|---|
| **where** | the span, as a caret under the source line |
| **expected** | what the language required there |
| **actual** | what the source says instead |
| **fix** | the edit that resolves it |

An error that is not a mismatch drops `expected` and `actual` — a duplicate
declaration has no "expected". It never drops `fix`. A diagnostic that cannot
say what to do about it is not finished, and the reject corpus in
`cli/tests/reject/` checks that case by case.

Every compile error carries a code in brackets after the message, and every code
has a page. Read one with `buri docs error <code>`, or list them all with
`buri docs error`. Each page holds a program that provokes the error, and the
test suite checks that it still does. `buri lint` findings carry a code the same
way, and the lints have their own pages.

### `--error-format=json`

For editors, continuous integration, and coding agents. One JSON object per
diagnostic, one per line, on stderr:

```
buri build //... --error-format=json
```

```json
{"severity":"error","message":"this `match` does not cover `.Empty`","location":{"file":"cmd/shapes/main.buri","line":8,"column":3,"endLine":11,"endColumn":4,"text":"  match (s) {","label":"not covered"},"fix":"add an arm for `.Empty`, or a `_` arm for everything left","notes":["every `match` must cover its scrutinee's type"],"related":[]}
```

| Field | |
|---|---|
| `severity` | `error`, `warning`, or `note` |
| `message` | the one-line summary |
| `code` | the lint name, on lint findings only |
| `location` | `file`, `line`, `column`, `endLine`, `endColumn`, the source `text` of that line, and an optional `label`. `null` where the diagnostic is about the invocation rather than a place in a file |
| `expected`, `actual` | present on a mismatch |
| `notes` | background, in order |
| `fix` | the edit to make. Always present |
| `related` | other locations, each shaped like `location` |

Lines are independent, so you can stream them. A missing field means "not
applicable" rather than "empty", which is why it is left out instead of set to
`null`.
