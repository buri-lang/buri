# The build model

A monorepo build system for Buri: `BUILD.buri` files in textproto, one CLI that
builds, tests, lints, formats, and generates build files, hermetic actions, and
an incremental cache keyed on content rather than on timestamps.

To learn the same thing by writing a repository, start with
[using the build system](../../guides/build-system.md).

| Document | What it covers |
|---|---|
| [`build-files.md`](./build-files.md) | Packages, labels, the `library` and `binary` rules, visibility |
| [`libraries.md`](./libraries.md) | `lib.buri` as the only public surface, re-exports, import resolution |
| [`tags.md`](./tags.md) | Build outputs, tags and the policy attached to them, platform restrictions |
| [`testing.md`](./testing.md) | The `test` declaration, the test platform, what a test can reach |
| [`repo-config.md`](./repo-config.md) | `REPO.buri`: the tag vocabulary and the lint policy |
| [`cli/`](../cli/) | `buri build`, `test`, `run`, `format`, `lint`, `gen`, `query` |
| [`hermeticity.md`](./hermeticity.md) | Sandboxing, action graph, cache keys, incrementality |
| [`schema/build.proto`](../schema/build.proto) | The normative schema for `BUILD.buri` |
| [`schema/repo.proto`](../schema/repo.proto) | The normative schema for `REPO.buri` |
| [`example/`](../../../../tests/example/) | A complete worked monorepo — the snippets in these pages are from it |

## The shape of a repository

Five rules give every Buri repository its layout, and everything else follows
from them:

- **A directory with a `BUILD.buri` is a package.** Subdirectories without one
  belong to the nearest ancestor package, so `posting/rules.buri` is part of
  `//lib/ledger`.
- **`lib.buri` is a library's whole public surface.** No other target can import
  a name that `lib.buri` leaves out, however public that name is inside the
  library.
- **`main.buri` is a compilation entry point** and exports `main`. Its build
  rule declares which outputs to produce: a Linux binary, a macOS binary, a JS
  module, or several at once.
- **Tests live in `test/` and see only the target's surface.** A library's
  tests import `//lib/money`, the same name a dependent writes. Fixtures a
  library offers *to other people's tests* live in `testing/`, and that path
  segment is what stops production code from importing them.
- **Everything is declared.** Sources, test sources, dependencies, outputs,
  visibility, tags. No globs, no discovery. A file on disk that no rule lists is
  an error.

## Tags, in one paragraph

A tag is a label saying what the code *is*, and it means the same thing on a
library and on a binary. `REPO.buri` declares once, on the tag itself, what
follows from wearing it. `forbids` names tags that may not appear anywhere in
the same dependency closure. `requires` whitelists the platforms the code may
build for. [`tags.md`](./tags.md) has the rules and the error messages.

## What the language buys the build system

The two designs lean on each other more than most:

| Language property | What the build system gets |
|---|---|
| Mandatory top-level signatures | The build hashes a library's *interface* without compiling its bodies. Editing a private function invalidates no dependent's typecheck. |
| Modules check independently | Compile actions within a package parallelize, with no ordering constraint beyond the dep graph. |
| No macros, no reflection, no conditional compilation | How you configure the build never changes what a source file means, so a cache key is (sources, dependencies, platform, build mode) and nothing else. Tags never enter it. |
| Effects arrive as bounds on `ctx` | The type system delivers hermeticity, not a sandbox. A test that never passes a `Net`-bounded context cannot reach the network. |
| `Result` is must-use | A `Result` a test forgets to check does not compile, so a test cannot silently pass. |
| No relative module paths | Moving a file does not change its imports, so `buri gen` rewrites a build file without touching source. |
| No mutation, no global state | Nothing observes test order, so the runner shards and reorders freely. |
| Circular imports are already an error | Package cycles are the same rule one level up, with the same diagnostic shape. |

## Still open

- **External repositories.** The `@repo//pkg` label syntax is reserved but
  unimplemented. Today your only sources are your own repository and `core/*`.
- **Remote caching and execution.** Not specified. The action keys in
  [`hermeticity.md`](./hermeticity.md) leave room for both: adding one changes
  the transport, not the semantics.
