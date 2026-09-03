# The CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, serves this documentation, and hosts the
language server. There is no second tool to install, no package manager, no task
runner, and no configuration of the CLI itself beyond
[`repo-config.md`](./build/repo-config.md).

Every command's synopsis and flag table is generated from the table that
dispatches it, so neither can describe a flag the binary does not accept nor
omit one it does. What follows here is what all of them share: how a target is
named, what the two global flags do, what the exit codes mean, and the shape of
a diagnostic.

## Naming targets

Target arguments accept labels and patterns (`//lib/money`, `//cmd/server`,
`//lib/...`, `//...`). A label names a package and every target in it. With no
argument, commands operate on the whole repository — bare is `//...`, and the
directory you happen to be standing in is not part of what a command means. All
commands are safe to run concurrently; a file lock serializes cache writes.

## The two global flags

`--color=never` drops the ANSI escapes. `--error-format=json` emits diagnostics
as one JSON object per line, and implies `--color=never`. Every other flag
belongs to a command and is listed with it.

## Exit codes

`0` success · `1` the thing you asked about is wrong · `2` the thing you asked
*with* is wrong.

A finding is `1`, and so is a compile error, a failing test, and a syntax error
in a source file: each is the answer to the question you asked. `2` is the run
that could not start — a target pattern that names nothing, a flag that does not
exist, a build file that does not parse.

## Diagnostics

Every diagnostic answers four questions, in a fixed order, so neither a person
nor a program has to infer any of them:

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
| **fix** | the concrete edit that resolves it |

`expected` and `actual` are omitted where the error is not a mismatch — a
duplicate declaration has no "expected" — but `fix` never is. A diagnostic that
cannot say what to do about it is not finished, and the reject corpus in
`cli/tests/reject/` asserts that case by case.

Every compile error carries a code in brackets after the message, and every code
has a page: `buri docs error <code>`, or `buri docs error` for the list. Each of
those pages carries a program that provokes the error, and the test suite checks
that it still does. What `buri lint` reports carries a code the same way, and
the lints have their own pages.

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

Lines are independent, so a consumer can stream them. Absent fields mean "not
applicable" rather than "empty", which is why they are omitted rather than
`null`.
