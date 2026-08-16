# The `buri` CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, and hosts the language server. There is no
second tool to install, no package manager, no task runner, and no configuration
of the CLI itself beyond [`REPO.buri`](./cli/src/docs/build/repo-config.md).

```
buri build   [targets]   compile
buri test    [targets]   compile and run test suites
buri run     <target>    build one binary and execute it
buri format  [paths]     format .buri sources and BUILD.buri files
buri lint    [targets]   static checks beyond type checking
buri gen     [targets]   regenerate sources/deps in existing BUILD.buri files
buri query   <expr>      ask about the build graph
buri clean               drop the local cache
buri lsp                 language server, over stdio
buri version             toolchain version and the REPO.buri pin
```

Target arguments accept labels and patterns (`//lib/money`,
`//cmd/server`, `//lib/...`, `//...`). A label names a package and every
target in it. With no
argument, commands operate on the package containing the working directory. All
commands are safe to run concurrently; a file lock serializes cache writes.

Two flags work on every command: `--color=never` drops the ANSI escapes, and
`--error-format=json` emits diagnostics as one JSON object per line. See
[Diagnostics](#diagnostics).

## `build`

```
buri build //...
buri build //cmd/server --output=linux/x86_64
buri build //cmd/server --release
```

Builds every requested target for every platform its `outputs` declare.
`--output` selects one. Artifacts land in
`.buri/out/<platform>/<package>/<artifact>`, where `<artifact>` is the package's
directory name unless the output overrides it with `artifact_name`, and a
convenience symlink `out/` points at the most recent:

```
.buri/out/linux-x86_64/cmd/server/server
.buri/out/js/cmd/web/web.mjs
```

Tags are not in the path, because they are not in the cache key: a tag decides
whether a build is permitted, never what it produces.

`--release` and `--debug` are flags on the command rather than repository
configuration, are part of the cache key, and default to `--debug`.

## `test`

```
buri test //...
buri test //lib/money --filter=pads
buri test //... --output=js
buri test //lib/money --accept   # update declared golden files
```

Covered in [`TESTING.md`](./cli/src/docs/build/testing.md). A suite whose inputs are unchanged is
not re-run and reports as cached; `--force` re-runs anyway, which is the honest
way to check that a suite is not accidentally depending on the cache.

## `run`

```
buri run //cmd/server
buri run //cmd/server -- --port=8080
```

A package holds at most one binary, so a label is enough to name it. Builds it
for the host configuration and executes it — **outside** the
sandbox, with the real environment and the real filesystem. That is the point of
`run`: it is the one command that produces a program with authority. Everything
before it in the pipeline is hermetic. Arguments after `--` go to the program.

## `format`

```
buri format                 format the whole repository
buri format --check         exit non-zero on any file that would change
buri format lib/money       format a subtree
```

Formats `.buri` sources and `BUILD.buri` files, with no options and no
configuration file. For build files: one field per line, two-space indent,
trailing commas, `sources` and `dependencies` sorted, `library` before `binary`,
comments kept with the field beneath them.

This is the same normalization `buri gen` applies, so the two commands never
disagree about a file.

`buri format --check` is the CI form. There is nothing to configure, so there is
nothing to argue about, and a formatter with options is a formatter whose output
is a repository decision.

## `lint`

```
buri lint //...
buri lint //lib/money
buri lint //... --fix      apply the findings that have one mechanical answer
```

Checks that type checking does not cover. Every finding names the edit that
resolves it, and by default none are applied — a report you have to read is
the point.

`--fix` applies the subset whose answer is mechanical, and only that subset:
a build file that disagrees with the code is handed to `buri gen`, an unused
import is deleted, and everything else is reported. A finding that needs a
judgement call — which edge of a `dep-cycle` to cut, which of two tags a target
should not have — is never guessed at, because a tool that picks one is not
fixing the finding, it is deleting the policy that raised it.

Build-graph rules — always errors, not configurable:

| | |
|---|---|
| `undeclared-source` | A `.buri` file in a package that no rule lists. |
| `duplicate-source` | A file listed by two rules. |
| `missing-dep` | Use of a library that is not in `dependencies` — by import, or by a method call resolving into it. |
| `unused-dep` | A `dependencies` entry no source uses. |
| `dep-cycle` | A cycle between packages. |
| `circular-import` | A cycle between *modules*, which is the same rule one level down. The message names the whole cycle, because any one edge of it looks fine alone. |
| `no-such-module` | A path that names no module. There are two kinds and no others: `core/...` and `//...`. |
| `module-outside-repository` | A `//...` path used where there is no repository to be relative to. |
| `host-import` | An import of `core/host` from a module other than the one exporting `main`. The context `main` builds is the program's whole effect budget; a second module able to import `core/host` would be a second place authority enters. |
| `internal-import` | An import of a module internal to another package. |
| `binary-entry-import` | An import of a binary's entry point from outside that binary's own test sources. |
| `test-only-import` | A non-test source importing a path with a `testing` segment. |
| `visibility-violation` | A dependency the target is not visible to. |
| `tag-violation` | Two tags that forbid each other in one dependency closure. |
| `platform-violation` | A target in the closure that does not admit the platform being built. |
| `unknown-tag` | A `tags` entry naming no `tag` block in `REPO.buri`. Suggests the nearest declared name. |

Style and hygiene rules:

| | Severity |
|---|---|
| `unreachable-export` | error — a module-level `export` that nothing in the library imports and `lib.buri` does not re-export |
| `unused-import` | error — an imported name that appears nowhere else in the module |
| `discarded-result` | warn — a call to `core/result.ignore`, the greppable escape hatch of [`SPEC.md` §6.8](./SPEC.md) |
| `empty-test-suite` | warn — a `test` block with no `sources` |
| `test-without-assertion` | warn — a `test` from which nothing reachable calls `core/testing/assert` |

Three of these read differently than you might expect, and the reasons are the
same reason in three shapes — a rule that fires on the wrong thing is worse
than no rule:

- `unused-import` is **syntactic**. A name counts as used if it appears as an
  identifier token anywhere outside the import statements. That
  over-approximates use, which is the safe direction at error severity: a
  shadowed binding with the same spelling silences the finding rather than
  producing a wrong one.
- `discarded-result` cannot be about `let _ = <Result>`, because that is
  already a hard type error — `result-discarded`, in the error catalog. The only
  way a `Result` is dropped on purpose is `ignore`, so that is what the rule
  reports — it is the grep, run for you.
- `test-without-assertion` is **transitive**. Read as "the body contains no
  `assert`" it fires on every test that asserts through a helper, which is most
  of the ones worth writing. A test passes the rule if anything reachable from
  it calls into `core/testing/assert`.

Import order is not a lint. `buri format` sorts imports, so an unsorted import
run is not a finding to report — it is a file that has not been formatted.

**None of this is configurable.** There is no `lint` block in `REPO.buri`, no
per-file suppression comment, and no way to promote or silence a check for one
repository. A configurable linter makes "does this code pass" a question you
cannot answer from the code, and an `allow` list is how a rule that should have
been argued about once gets turned off quietly instead. A check that is wrong
often enough to want silencing is a check to change here, in the catalogue,
where the argument happens once.

## `gen`

```
buri gen //...
buri gen //lib/money
buri gen --check          exit non-zero if any build file is out of date
```

Rewrites, in every requested package's existing `BUILD.buri`:

- `sources` — every `.buri` file in the package, excluding `lib.buri`,
  `main.buri`, and anything under `test/` or `testing/`, assigned to a rule
  (see below);
- `dependencies` — every library those sources use, minus the co-located library: the
  `//` imports, plus the libraries reached by method resolution, which the tool
  can compute because resolution is a single lookup;
- `test.sources` — every `.buri` file under `test/`;
- `testing.sources` — every `.buri` file under `testing/` but `testing/lib.buri`;
- `testing.dependencies` — the libraries those modules use;
- `test.dependencies` — the libraries the test sources use, minus the target under test
  and its `dependencies`.

and **no other field's contents**. `tags`, `platforms`, `timeout_seconds`,
`visibility`, `outputs`, `test.data`, `test.platforms`, and every comment come
back saying exactly what they said. A field the tool manages is replaced whole
rather than merged, so hand-editing `sources` is pointless and hand-editing
`tags` is expected.

What `gen` may change everywhere is **formatting**: it leaves the file exactly
as `buri format` would, so a hand-written `tags` list can come back rewrapped,
re-indented, or moved to one-field-per-line, and a comment stays with the field
beneath it. Running `gen` and running `format` never fight over a file.

That split is the whole design of the command. `buri gen` writes the fields that
*restate the sources* — where a file lives, what it imports — and it is safe to
run over the entire repository precisely because it cannot write the fields that
*constrain* them. `tags` and `platforms` are the ones that matter most: a tag
decides what a target may be linked with, `platforms` decides where it may be
built, neither is derivable from an import graph, and a tool that dropped a
`tags` entry while tidying `sources` would turn `buri gen //...` into a way to
quietly delete policy. The same holds for a rule `gen` finds empty — the tags
stay even if every source is removed. `visibility` and `outputs` are preserved
for the same reason and a plainer one besides: nothing in the code says who
*ought* to be allowed to depend on a library, or which platforms you *want* to
ship, so there is nothing for the tool to derive them from.

**A `BUILD.buri` must already exist, with the rule blocks.** `buri gen` never
creates a build file and never adds a rule. Deciding that a directory is a
library — that it has an API, an owner, a visibility, a tag — is a design
decision, and inferring it from the presence of a `lib.buri` is how a repository
acquires two hundred libraries nobody chose. An empty rule is enough to start:

```textproto schema=build
library {}
```

```
$ buri gen //lib/money
updated lib/money/BUILD.buri
  + sources: cents.buri, parse.buri
  + test.sources: test/cents.buri, test/parse.buri
```

In a package with both rules, `gen` needs to know which rule a new file belongs
to. The rules, in order:

1. A file already listed in a rule's `sources` stays there.
2. A file reachable by imports from `main.buri` and not from `lib.buri` goes to
   the binary.
3. A file reachable from `lib.buri` goes to the library.
4. A file reachable from neither, or from both, is an error that names the file
   and asks you to place it. Guessing here would silently move code across a
   boundary that exists to be explicit.

`buri gen --check` in CI keeps build files honest without anyone having to
remember the command.

## `query`

```
buri query 'deps(//cmd/server)'                transitive deps
buri query 'rdeps(//lib/money)'                 who depends on this
buri query 'path(//cmd/web, //lib/store)'      why is this linked in
buri query 'tags(//lib/store)'                  every tag in its closure
buri query 'platforms(//lib/store)'            the platforms it can be built for
buri query 'sources(//lib/money)'              files, as the build sees them
```

`path` is the one that earns its place: the answer to "why does the JS build
pull in the database layer" is an edge, and printing it is faster than reading
build files.

```
$ buri query 'path(//cmd/web, //lib/store)'
//cmd/web
  -> //lib/ledger           cmd/web/BUILD.buri:7
  -> //lib/store            lib/ledger/BUILD.buri:9
```

## `clean`

```
buri clean                drop .buri/cache and .buri/out
buri clean --outputs      drop .buri/out only
```

Rarely needed — the cache is keyed on content, so a stale entry is a bug rather
than a fact of life ([`HERMETICITY-AND-CACHING.md`](./cli/src/docs/build/hermeticity.md)).
Reaching for `buri clean` to fix a build is worth reporting.

## `lsp`

Language server over stdio, backed by the same analysis the compiler runs, and
aware of the build graph: completion inside a `from "//` import offers the
libraries in `dependencies`, hovering a label shows the target, and an import with no
matching `dependencies` entry comes with a "add to `dependencies`" code action that edits the
`BUILD.buri`.

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

### `--error-format=json`

For editors, CI, and coding agents. One JSON object per diagnostic, one per
line, on stderr:

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
`null`. `--error-format=json` implies `--color=never`.

## Exit codes

| | |
|---|---|
| 0 | Success. For `test`, every test passed. |
| 1 | Build, lint, or test failure — the thing you asked about is wrong. |
| 2 | Malformed invocation, unparseable `BUILD.buri` or `REPO.buri`, toolchain hash mismatch — the thing you asked *with* is wrong. |
