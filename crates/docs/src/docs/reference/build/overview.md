# The build model

Buri's monorepo build system: `BUILD.buri` files in textproto, one CLI that
builds, tests, lints, formats, and generates build files, hermetic actions, and
a cache keyed on content, not timestamps.

To learn it by writing a repository, start with
[using the build system](../../guides/build-system.md).

| Document | What it covers |
|---|---|
| [`build-files.md`](./build-files.md) | Packages, labels, the `library` and `binary` rules, visibility |
| [`libraries.md`](./libraries.md) | `lib.buri` as the only public surface, re-exports, import resolution |
| [`tags.md`](./tags.md) | Build outputs, tags and the policy attached to them, platform restrictions |
| [`testing.md`](./testing.md) | The `test` declaration, the test platform, what a test can reach |
| [`repo-config.md`](./repo-config.md) | `REPO.buri`: the tag vocabulary and the lint policy |
| [`cli/`](../cli/) | `buri build`, `test`, `run`, `format`, `lint`, `gen`, `query` |
| [`hermeticity.md`](./hermeticity.md) | Hermeticity, action graph, cache keys, incrementality |
| [`schema/build.proto`](../schema/build.proto) | The normative schema for `BUILD.buri` |
| [`schema/repo.proto`](../schema/repo.proto) | The normative schema for `REPO.buri` |
| [`example/`](../../../../../../cli/tests/example/) | A complete worked monorepo — the snippets in these pages are from it |

## The shape of a repository

- **A directory with a `BUILD.buri` is a package.** Subdirectories without one
  belong to the nearest ancestor package, so `posting/rules.buri` is part of
  `//lib/ledger`.
- **`lib.buri` is a library's whole public surface.** No other target can import
  a name `lib.buri` leaves out.
- **`main.buri` is a compilation entry point** and exports `main`. Its build
  rule declares the outputs: a Linux binary, a macOS binary, a JS module, or
  several.
- **Tests live in `test/` and see only the target's surface.** A library's
  tests import `//lib/money`, the same name a dependent writes. Fixtures a
  library offers *to other people's tests* live in `testing/`, and that path
  segment stops production code from importing them.
- **Each directory under `apps/` is one app.** No app reaches another's
  packages, and nothing outside `apps/` reaches an app's. Shared code lives in
  `libs/`.
- **Everything is declared**: sources, test sources, dependencies, outputs,
  visibility, tags. No globs, no discovery. A file on disk that no rule lists is
  an error.

## Tags, in one paragraph

A tag says what code *is*, on a library or a binary alike. `REPO.buri` declares
what follows from wearing it. `forbids` names tags that may not appear in the
same dependency closure, and platforms the code may not be built for.
`requires` whitelists the platforms it may build for. [`tags.md`](./tags.md)
has the rules and the error messages.

## What the language buys the build system

| Language property | What the build system gets |
|---|---|
| Mandatory top-level signatures | The build hashes a library's *interface* without compiling its bodies. Editing a private function invalidates no dependent's typecheck. |
| Modules check independently | Compile actions within a package parallelize, with no ordering constraint beyond the dep graph. |
| No macros, no reflection, no conditional compilation | Build configuration never changes what a source file means, so a cache key is (sources, dependencies, platform, build mode) and nothing else. Tags never enter it. |
| Effects arrive as bounds on `ctx` | The type system delivers hermeticity, not a sandbox. A test that never passes a `Network`-bounded context can't reach the network. |
| `Result` is must-use | A `Result` a test forgets to check doesn't compile, so a test can't silently pass. |
| No relative module paths | Moving a file doesn't change its imports, so `buri gen` rewrites a build file without touching source. |
| No mutation, no global state | Nothing observes test order, so the runner shards and reorders freely. |
| Circular imports are already an error | Package cycles are the same rule one level up, with the same diagnostic shape. |

## Still open

- **External repositories.** The `@repo//pkg` label syntax is reserved but
  unimplemented. Your only sources are your own repository and `core/*`.
- **Remote caching and execution.** Not specified. The action keys in
  [`hermeticity.md`](./hermeticity.md) leave room for both as a transport
  change, not a semantic one.
