# The CLI

One binary builds, runs, tests, formats, lints, generates build files, queries
the graph, serves this documentation and hosts the language server. The only
configuration is [`repo-config.md`](./build/repo-config.md).

Each command's synopsis and flag table come from the code that dispatches it, so
they always match the binary. This page covers what every command shares.

## Naming targets

Target arguments take labels and patterns: `//lib/money`, `//cmd/server`,
`//lib/...`, `//...`. A label names a package and every target in it. With no
argument a command covers `//...`; your current directory never changes what a
command means. Commands can run concurrently.

`buri format` also takes a path, since it formats files no build file declares.

## The two global flags

`--color=never` drops the ANSI escapes. `--error-format=json` prints diagnostics
as one JSON object per line, and implies `--color=never`. Every other flag
belongs to one command, and its page lists it.

## Exit codes

`0` success · `1` the thing you asked about is wrong · `2` the thing you asked
*with* is wrong.

A lint finding, compile error, failing test or syntax error in a source exits
`1`. `2` means the run never started: a target pattern that names nothing, an
unknown flag, a build file that doesn't parse.

When the reader of stdout goes away, as in `buri test | head -1`, the command
stops at once and silently, killed by `SIGPIPE` like any Unix tool. Shells
report that as `141`. A program `buri` builds isn't killed: its print answers
`.Err`, on every backend.

## Diagnostics

Every diagnostic answers four questions in the same order:

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

An error that isn't a mismatch, like a duplicate declaration, drops `expected`
and `actual`. Every diagnostic has a `fix`.

Every compile error carries a code in brackets after the message, and every code
has a page: `buri docs error <code>` reads one, `buri docs error` lists them.
Each page holds a program that provokes the error. `buri lint` findings carry
codes too.

### `--error-format=json`

One JSON object per diagnostic, one per line, on stderr:

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
applicable".
