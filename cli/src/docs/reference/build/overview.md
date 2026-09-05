# The build model

A monorepo build system for Buri: `BUILD.buri` files in textproto, one CLI that
builds, tests, lints, formats, and generates build files, hermetic actions, and
an incremental cache keyed on content rather than on timestamps.

This page is the model the rest of the build reference is written against: the
rules a repository's shape follows from, what the language contributes, and
which questions are decided. To learn the same thing by writing one, start with
[using the build system](../../guides/build-system.md).

**This is a design document, not an implementation**, in the same sense that
[the language specification](../../language/introduction.md) is. It is written
to be specific enough to argue with.

| Document | What it covers |
|---|---|
| [`build-files.md`](./build-files.md) | Packages, labels, the `library` and `binary` rules, visibility |
| [`libraries.md`](./libraries.md) | `lib.buri` as the only public surface, re-exports, import resolution |
| [`tags.md`](./tags.md) | Build outputs, tags and the policy attached to them, platform restrictions |
| [`testing.md`](./testing.md) | The `test` declaration, the test platform, what a test can reach |
| [`repo-config.md`](./repo-config.md) | `REPO.buri`: the tag vocabulary, the lint policy, and what a repository-wide file deliberately does not hold |
| [`cli/`](../cli/) | `buri build`, `test`, `run`, `format`, `lint`, `gen`, `query` |
| [`hermeticity.md`](./hermeticity.md) | Sandboxing, action graph, cache keys, incrementality |
| [`schema/build.proto`](../schema/build.proto) | The normative schema for `BUILD.buri` |
| [`schema/repo.proto`](../schema/repo.proto) | The normative schema for `REPO.buri` |
| [`example/`](../../../../tests/example/) | A complete worked monorepo — the snippets in these pages are from it |

## The shape of a repository

Five rules produce the layout every Buri repository has, and everything else in
these documents follows from them:

- **A directory with a `BUILD.buri` is a package.** Subdirectories without one
  belong to the nearest ancestor package, so `posting/rules.buri` is part of
  `//lib/ledger`. Organize freely; a directory is not a unit of anything.
- **`lib.buri` is a library's whole public surface.** It exports, or
  re-exports, everything another target can see. A name that does not appear in
  `lib.buri` cannot be imported from outside the library, however public it is
  within it.
- **`main.buri` is a compilation entry point** and exports `main`. Its build
  rule declares which outputs to produce — a Linux binary, a macOS binary, a JS
  module, or several at once.
- **Tests live in `test/` and see only the target's surface.** A library's
  tests import `//lib/money` — its surface, the same name a dependent
  writes — and not the files behind it. A library's fixtures *for other
  people's tests* live in `testing/`, and that segment in a path is what makes
  them unimportable from production code.
- **Everything is declared.** Sources, test sources, dependencies, outputs,
  visibility, tags. No globs, no discovery, no implicit `..` walk. A file on
  disk that no rule lists is an error.

## Tags, in one paragraph

`tags` are labels saying what the code *is*, and mean the same thing on a
library and on a binary. What follows from a label is declared once in
`REPO.buri`, on the tag itself: `forbids` names tags that may not appear
anywhere in the same dependency closure, and `requires` whitelists the
platforms the code may be built for. [`tags.md`](./tags.md) has the rules, the
reasoning, and the error messages.

## What the language buys the build system

The two designs lean on each other more than is usual, and the places where
they meet are the ones worth reviewing hardest:

| Language property | What the build system gets |
|---|---|
| Mandatory top-level signatures | A library's *interface* hash is derivable without compiling its bodies. Editing a private function does not invalidate a single dependent's typecheck. |
| Modules check independently | Compile actions within a package parallelize with no ordering constraints beyond the dep graph. |
| No macros, no reflection, no conditional compilation | A source file's meaning does not depend on how the build was configured, so a cache key is (sources, dependencies, platform, build mode) and nothing else — tags never enter it. |
| Effects arrive as bounds on `ctx` | Hermeticity is a type-system property rather than a sandbox one. A test whose calls never passed a `Net`-bounded context cannot reach the network, so there is nothing for an operating-system confinement to confine — and the toolchain applies none. |
| `Result` is must-use | A `Result` a test forgets to check does not compile, so a test cannot silently pass. |
| No relative module paths | A file's imports do not change when it moves, so `buri gen` can rewrite a build file without rewriting source. |
| No mutation, no global state | Test order is not observable; the runner may shard and reorder freely. |
| Circular imports are already an error | Package cycles are the same rule one level up, with the same diagnostic shape. |

## What is deliberately absent

- **No expression language and no globs in `BUILD.buri`.** Build files are
  data; `buri gen` writes most of them for you. The reasoning is in
  [`build-files.md`](./build-files.md#the-file).
- **No user-defined rules or toolchains.** Two rule kinds, and the compiler is
  the only tool.

## Settled, and what each one costs

These were the open questions of the first draft. They are decided, and each
cost is recorded because a later change should have to argue with it. The page
that owns each decision has the argument.

- **`test` and `assert` are reserved words** ([`language/programs.md`
  §11.2](../../language/programs.md)). *Costs:* no function may be named
  `test`, no namespace `assert`.
- **One library per package.** *Buys:* `//lib/money` names a directory, a
  library, and a module at once, so no label ever needs a `:target`. *Costs:*
  splitting a library in two is a directory move.
- **`lib.buri` may hold logic** ([`libraries.md`](./libraries.md#the-re-export-declaration)).
  *Costs:* the surface is no longer auditable by shape — a reader has to
  notice `export fn` alongside the re-exports.
- **Tags are labels, and the policy lives on the tag declaration**
  ([`tags.md`](./tags.md)). *Costs:* nothing can require a binary to take a
  position, since a `forbids` rule fires only once two tags collide.
  Enforcement is opt-in, traded for there being no resolution algorithm.
- **Golden values live in the suite's own source**
  ([`testing.md`](./testing.md#test-data-and-golden-files)). *Costs:* an editor,
  rather than the runner, is what rewrites one.
- **Test-only code is marked by its path, not by a field**
  ([`libraries.md`](./libraries.md#the-testing-surface)). *Costs:* `testing`
  is a reserved directory name, and there is nothing to grep for in a build
  file.

## Still open

- **External repositories.** The `@repo//pkg` label syntax is reserved and
  unimplemented; today the only sources are your repository and `core/*`.
- **Remote caching and execution.** Not specified. The action-key design in
  [`hermeticity.md`](./hermeticity.md) is chosen so
  that adding one is a transport change rather than a semantic one.
