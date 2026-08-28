# The `buri` CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, serves this documentation, and hosts the
language server. There is no second tool to install, no package manager, no task
runner, and no configuration of the CLI itself beyond
[`repo-config.md`](./repo-config.md).

Every command has a page of its own, and the synopsis and flag table on each are
generated from the same table that dispatches, so a page cannot describe a flag
the binary does not accept nor omit one it does. This page is what the commands
share: how a target is named, how a finding is reported, and the catalogue of
findings `buri lint` can raise.

| Command | What it does | Its page |
|---|---|---|
| `buri init [directory]` | Writes a new repository: a library, a binary, a test, and the agent skills. | `buri docs cli init` |
| `buri build [targets]` | Compiles the targets you name. | `buri docs cli build` |
| `buri test [targets]` | Compiles and runs test suites. | `buri docs cli test` |
| `buri run <target>` | Builds one binary and executes it. | `buri docs cli run` |
| `buri format [paths]` | Formats `.buri` sources and build files. | `buri docs cli format` |
| `buri lint [targets]` | Static checks beyond type checking. | `buri docs cli lint` |
| `buri gen [targets]` | Regenerates the fields of a `BUILD.buri` that restate the sources. | `buri docs cli gen` |
| `buri query <expr>` | Asks about the build graph. | `buri docs cli query` |
| `buri docs [topic]` | Serves the language, the build system, and this CLI. | `buri docs cli docs` |
| `buri add-skills [directory]` | Writes the agent skills this toolchain ships into `.claude/skills`. | `buri docs cli add-skills` |
| `buri lsp` | The language server, over standard input and output. | `buri docs cli lsp` |
| `buri clean` | Drops the local cache. | `buri docs cli clean` |
| `buri version` | The toolchain version, and with `--verbose` its executable's hash. | `buri docs cli version` |

Target arguments accept labels and patterns (`//lib/money`, `//cmd/server`,
`//lib/...`, `//...`). A label names a package and every target in it. With no
argument, commands operate on the whole repository — bare is `//...`, and the
directory you happen to be standing in is not part of what a command means. All
commands are safe to run concurrently; a file lock serializes cache writes.

Two flags work on every command: `--color=never` drops the ANSI escapes, and
`--error-format=json` emits diagnostics as one JSON object per line. See
[Diagnostics](#diagnostics).

## `lint`

`buri lint` checks what type checking does not cover. Every finding names the
edit that resolves it, and carries a stable code so a report can be grepped and
a check can be talked about by name. `buri docs cli lint` covers `--fix`; the
catalogue is here, because it is the list a repository argues about rather than
a property of the command.

**Every finding in it is a warning.** One catalogue, one severity, the same in
every repository, which is why no table below carries a column saying so — a
column whose every row reads the same word is one nobody reads twice. What
varies is not what a finding is but what it costs. `buri lint` exits nonzero if
it reports anything at all, because invoking the linter is itself the request to
be told and a report that exits zero is one no script can act on. `buri build`
and `buri test` run the catalogue only where `REPO.buri` asks them to, and fail
on what it finds only where it asks for that as well — both are fields on the
[`lint` block](./repo-config.md#lint), and both default to today's behavior.

Build-graph rules:

| | |
|---|---|
| `unused-library` | A `.buri` or `.proto` file in a package that no rule lists, so it belongs to no library and no binary and nothing builds it. The fix names the field it belongs in — `sources` for one, `proto_sources` for the other. |
| `duplicate-source` | A file listed by two rules. |
| `entry-point-listed` | An entry point written into a `sources` list. `lib.buri`, `main.buri` and `testing/lib.buri` are named by the rule kind, so listing one says nothing the rule had not already said. |
| `undeclared-testing-surface` | A `testing/` directory with no `testing` block to declare it. The block is what puts the surface in the build; without it nothing compiles `testing/lib.buri` and no dependent can name it. |
| `missing-dep` | Use of a library that is not in `dependencies` — by import, or by a method call resolving into it. |
| `dep-cycle` | A cycle between packages. |
| `circular-import` | A cycle between *modules*, which is the same rule one level down. The message names the whole cycle, because any one edge of it looks fine alone. |
| `proto-circular-import` | The same cycle between `.proto` schemas, which import each other the way modules do. |
| `no-such-module` | A path that names no module. There are two kinds and no others: `core/...` and `//...`. |
| `module-outside-repository` | A `//...` path used where there is no repository to be relative to. |
| `host-import` | An import of `core/host` from a module other than the one exporting `main`. The context `main` builds is the program's whole effect budget; a second module able to import `core/host` would be a second place authority enters. |
| `host-not-granted` | A `main` binding an effect the output's platform does not grant — `Ui: host.ui` under `platform: JS`, `Net: host.net` under `platform: WEB`. A platform *is* the set of effects its host exports, so the name is simply not there, and the fix names the platforms that do grant it. |
| `internal-import` | An import of a module internal to another package's library. |
| `binary-internal-import` | A binary's source importing a module internal to the library beside it. The boundary is the *rule's*, not the directory's, so being in the same package is not enough. |
| `binary-source-import` | A library's source importing a module that belongs to the binary beside it. The binary depends on the library, not the other way round. |
| `binary-entry-import` | An import of a binary's entry point from outside that binary's own test sources. |
| `test-only-import` | A non-test source importing a path with a `testing` segment. |
| `test-internal-import` | A file listed in `test.sources` importing a module internal to the library it tests. A test reaches its library the way a dependent does. |
| `test-source-import` | An import of a file listed in `test.sources`. Test sources are compiled independently, so there is no module for one to name. |
| `visibility-violation` | A dependency the target is not visible to. |
| `tag-violation` | Two tags that forbid each other in one dependency closure. |
| `platform-violation` | A target in the closure that does not admit the platform being built. |
| `unsatisfiable-target` | A target whose dependency closure admits no platform at all, so there is no platform to build it for. Reported at the target itself rather than at whichever binary happens to reach it first. |
| `unknown-tag` | A `tags` entry naming no `tag` block in `REPO.buri`. Suggests the nearest declared name. |
| `proto-edition` | A `.proto` file declaring an edition other than `2026`. The fix names the one edition this reader implements. |
| `proto-syntax-declaration` | A `syntax = "proto2"` or `syntax = "proto3"` file. The fix is the migration to editions. |
| `proto-edition-missing` | A `.proto` file with no `edition` line at all, which every other tool reads as proto2. |
| `proto-schema` | A `.proto` file that is not a well-formed schema: a field number outside 1..536870911, an enum whose first value is not zero, an unclosed message. |
| `proto-unsupported` | A construct or a feature value the schema reader refuses, named: `service`, `extend`, `extensions`, `group`, `map<>`, `google.protobuf.Any`, `import public`, the removed `optional` and `required` labels, and the `features.…` values it cannot express — `LEGACY_REQUIRED`, `CLOSED`, `DELIMITED`, `NONE`, `LEGACY_BEST_EFFORT`. [`proto.md`](./proto.md) says why each one is out. |
| `proto-unknown-feature` | A `features.…` name the reader does not model at all, as against a value of a known one it refuses. Suggests the nearest name it knows. |
| `proto-unknown-type` | A field whose type names no message or enum, in this schema or in one it imports. |
| `proto-ambiguous-type` | A field whose type names a short name two imported schemas both claim. Which one it meant is not something import order should decide, so it is asked rather than guessed. |
| `proto-duplicate-type` | One fully-qualified name declared by two schemas. Reported whether or not anything uses it, and naming both files. |
| `proto-import-not-found` | An `import` inside a schema naming no file. The path is written from the repository root, the way protoc resolves one against `-I.`. |
| `proto-source-not-a-schema` | A `proto_sources` entry that is not a `.proto` file. |

Style and hygiene rules:

| | |
|---|---|
| `dead-code` | A module-level `export` that nothing in the library imports and `lib.buri` does not re-export, so nothing reaches it. |
| `unused-import` | An imported name that appears nowhere else in the module. |
| `discarded-result` | A call to `core/result.ignore`, the greppable escape hatch of [`SPEC.md` §6.8](../SPEC.md). |
| `empty-test-suite` | A `test` block with no `sources`. |
| `test-without-assertion` | A `test` from which nothing reachable calls `core/testing/assert`. |
| `test-title-newline` | A `test` title with a line break in it, which a report has to escape. |
| `duplicate-import` | One module named by two `import` statements, both naming names. |
| `unused-variable` | A `let` whose name nothing below it reads. |
| `warning-comment` | A comment carrying `TODO`, `FIXME` or `HACK`. |
| `too-many-parameters` | More than five parameters, counting neither `self` nor `ctx`. |
| `oversized-function` | A body more than forty lines from its opening brace to its closing one. |
| `deep-nesting` | A branch with more than four branches wrapped around it. |
| `ctx-rebinding` | A `let ctx = ...` where no context may be built, which binds the name a function's context arrives under to something else. |

Two findings belong to a `buri test` run rather than to the graph, and both are
about the suite as a whole rather than about one test in it:

| | |
|---|---|
| `test-timeout` | The suite ran past its `test { timeout_seconds }` and was killed, so no test in it has a result. |
| `platform-not-implemented` | A platform in `test { platforms }` that this toolchain cannot produce a binary for — no backend compiled in for it, no runtime archive, or no way to link it from this host. Distinct from `platform-violation`, which is the target refusing a platform it could otherwise be built for. |

Several of the lint rules read differently than you might expect, and the
reasons are the same reason in several shapes — a rule that fires on the wrong
thing is worse than no rule:

- `unused-import` is **syntactic**. A name counts as used if it appears as an
  identifier token anywhere outside the import statements. That
  over-approximates use, which is the safe direction for a rule nobody can turn
  off: a shadowed binding with the same spelling silences the finding rather
  than producing a wrong one.
- `discarded-result` cannot be about `let _ = <Result>`, because that is
  already a hard type error — `result-discarded`, in the error catalog. The only
  way a `Result` is dropped on purpose is `ignore`, so that is what the rule
  reports — it is the grep, run for you.
- `test-without-assertion` is **transitive**. Read as "the body contains no
  `assert`" it fires on every test that asserts through a helper, which is most
  of the ones worth writing. A test passes the rule if anything reachable from
  it calls into `core/testing/assert`.
- `duplicate-import` is about two **named** clauses. A statement carries one
  clause, so a namespace import and a named one cannot become a single
  statement, and a rule that asked them to would be asking for something the
  grammar does not have.
- `unused-variable` is about `let` and nothing else. A binding in a `match` arm
  or a lambda's parameter list is part of a shape being described rather than a
  name introduced for the lines below, and whether one is read is a different
  question.
- `warning-comment` reads the gaps between tokens, which is what makes it a rule
  about comments rather than about text: a `TODO` inside a string literal is
  inside a token and is never looked at. `XXX` is not one of the markers,
  because `\uXXXX` is how an escape is written in prose.
- `oversized-function`, `too-many-parameters` and `deep-nesting` carry the only
  numbers in the catalogue, and each sits where the whole of this repository's
  Buri stays under it — a `test` block is not a function for the first of them,
  an `else if` chain is one level for the last, and `self` and `ctx` are not
  parameters a caller assembled for the middle one.

Import order is not a lint. `buri format` sorts imports, so an unsorted import
run is not a finding to report — it is a file that has not been formatted.

No check here is configurable. `REPO.buri` has a `lint` block, and everything it
can say is about the catalogue as a whole rather than about one rule in it:
`check_during_build` runs these checks during `buri build` and `buri test`,
`fail_on_finding` makes what they report fail the command. Both only tighten.
There is no per-rule severity, no allow list, no per-directory exemption and no
per-file suppression comment, and
[`repo-config.md`](./repo-config.md#what-is-not-here) says why a linter you can
argue with is one whose verdict is no longer a fact about the code.

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
that it still does.

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
`null`. `--error-format=json` implies `--color=never`.
