# The `buri` CLI

One binary. It builds, runs, tests, formats, lints, generates build files,
answers questions about the graph, and hosts the language server. There is no
second tool to install, no package manager, no task runner, and no configuration
of the CLI itself beyond [`REPO.buri`](./REPO-CONFIG.md).

```
buri build   [targets]   compile
buri test    [targets]   compile and run test suites
buri run     <target>    build one binary and execute it
buri fmt     [paths]     format .buri sources and BUILD.buri files
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

## `build`

```
buri build //...
buri build //cmd/server --output=linux/x86_64
buri build //cmd/server --release
```

Builds every requested target for every platform its `outputs` declare.
`--output` selects one. Artifacts land in `.buri/out/<platform>/<package>/<name>`
and a convenience symlink `out/` points at the most recent:

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
buri test //... --shuffle=off
```

Covered in [`TESTING.md`](./TESTING.md). A suite whose inputs are unchanged is
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

## `fmt`

```
buri fmt                 format the whole repository
buri fmt --check         exit non-zero on any file that would change
buri fmt lib/money       format a subtree
```

Formats `.buri` sources and `BUILD.buri` files, with no options and no
configuration file. For build files: one field per line, two-space indent,
trailing commas, `sources` and `dependencies` sorted, rule blocks in the order `package`,
`library`, `binary`, comments kept with the field beneath them.

`buri fmt --check` is the CI form. There is nothing to configure, so there is
nothing to argue about, and a formatter with options is a formatter whose output
is a repository decision.

## `lint`

```
buri lint //...
buri lint //lib/money --fix
```

Checks that type checking does not cover. `--fix` applies the mechanical ones
(import sorting, unused imports, `buri gen`-able build file drift).

Build-graph rules — always errors, not configurable:

| | |
|---|---|
| `undeclared-source` | A `.buri` file in a package that no rule lists. |
| `duplicate-source` | A file listed by two rules. |
| `missing-dep` | Use of a library that is not in `dependencies` — by import, or by a method call resolving into it. |
| `unused-dep` | A `dependencies` entry no source uses. |
| `dep-cycle` | A cycle between packages. |
| `boundary-violation` | An import of a module internal to another package, or across a rule boundary within one. |
| `testonly-in-production` | A non-test source importing a path with a `testing` segment. |
| `visibility-violation` | A dependency the target is not visible to. |
| `tag-violation` | Two tags that forbid each other in one dependency closure. |
| `platform-violation` | A target in the closure that does not admit the platform being built. |
| `unknown-tag` | A `tags` entry naming no `tag` block in `REPO.buri`. Suggests the nearest declared name. |

Style and hygiene rules:

| | Severity |
|---|---|
| `unreachable-export` | error — a module-level `export` that nothing in the library imports and `lib.buri` does not re-export |
| `name-matches-directory` | warn — a target whose `name` is not its directory's |
| `unused-import` | error |
| `unsorted-imports` | warn |
| `discarded-result` | warn — `let _ =` on a `Result`, the greppable escape hatch of [`SPEC.md` §6.8](../SPEC.md) |
| `empty-test-suite` | warn — a `test` block with no `sources` |
| `test-without-assertion` | warn — a `test` declaration whose body contains no `assert` |

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

and rewrites **nothing else**. `name`, `tags`, `platforms`, `visibility`,
`outputs`, `test.data`, `timeout_seconds`, and every comment survive
byte-identical. Generated lists are sorted; a field the tool manages is replaced
whole rather than merged, so hand-editing `sources` is pointless and hand-editing
`tags` is expected.

**A `BUILD.buri` must already exist, with the rule blocks and their names.**
`buri gen` never creates a build file and never adds a rule. Deciding that a
directory is a library — that it has an API, an owner, a visibility, a
tag — is a design decision, and inferring it from the presence of a `lib.buri`
is how a repository acquires two hundred libraries nobody chose. A stub is
enough to start:

```textproto
library { name: "money" }
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

Output is textproto with `--output=proto`, for scripts.

## `clean`

```
buri clean                drop .buri/cache and .buri/out
buri clean --outputs      drop .buri/out only
```

Rarely needed — the cache is keyed on content, so a stale entry is a bug rather
than a fact of life ([`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md)).
Reaching for `buri clean` to fix a build is worth reporting.

## `lsp`

Language server over stdio, backed by the same analysis the compiler runs, and
aware of the build graph: completion inside a `from "//` import offers the
libraries in `dependencies`, hovering a label shows the target, and an import with no
matching `dependencies` entry comes with a "add to `dependencies`" code action that edits the
`BUILD.buri`.

## Exit codes

| | |
|---|---|
| 0 | Success. For `test`, every test passed. |
| 1 | Build, lint, or test failure — the thing you asked about is wrong. |
| 2 | Malformed invocation, unparseable `BUILD.buri` or `REPO.buri`, toolchain hash mismatch — the thing you asked *with* is wrong. |
