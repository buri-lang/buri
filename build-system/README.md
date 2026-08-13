# The Buri build system

A monorepo build system for Buri: `BUILD.buri` files in textproto, one CLI that
builds, tests, lints, formats, and generates build files, hermetic actions, and
an incremental cache keyed on content rather than on timestamps.

**This is a design document, not an implementation**, in the same sense that
[`SPEC.md`](../SPEC.md) is. It is written to be specific enough to argue with.

| Document | What it covers |
|---|---|
| [`BUILD-FILES.md`](./BUILD-FILES.md) | Packages, labels, the `library` and `binary` rules, visibility |
| [`LIBRARIES.md`](./LIBRARIES.md) | `lib.buri` as the only public surface, re-exports, import resolution |
| [`TAGS.md`](./TAGS.md) | Build outputs, dimensions, tag composition, repository-wide policy |
| [`TESTING.md`](./TESTING.md) | The `test` declaration, the test platform, what a test can reach |
| [`REPO-CONFIG.md`](./REPO-CONFIG.md) | `REPO.buri`: toolchain pin, dimensions, defaults, lint policy |
| [`CLI.md`](./CLI.md) | `buri build`, `test`, `run`, `fmt`, `lint`, `gen`, `query` |
| [`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md) | Sandboxing, action graph, cache keys, incrementality |
| [`schema/build.proto`](./schema/build.proto) | The normative schema for both file formats |
| [`example/`](./example/) | A complete worked monorepo — every snippet below is from it |

## The shape of a repository

```
REPO.buri                     # repository root, toolchain pin, tag vocabulary
lib/
  money/
    BUILD.buri                # declares //lib/money
    lib.buri                  # the library's entire public surface
    cents.buri                # internal
    parse.buri                # internal
    test/
      cents.buri              # tests, against lib.buri only
      parse.buri
  ledger/
    BUILD.buri
    lib.buri
    entry.buri
    posting/                  # a subdirectory, not a package: still //lib/ledger
      rules.buri
    test/
      ledger.buri
cmd/
  server/
    BUILD.buri                # declares //cmd/server
    main.buri                 # compilation entry point
    routes.buri
    test/
      routes.buri             # tests, against main.buri only
```

Five rules produce that shape, and everything else in these documents follows
from them:

1. **A directory with a `BUILD.buri` is a package.** Subdirectories without one
   belong to the nearest ancestor package, so `posting/rules.buri` is part of
   `//lib/ledger`. Organize freely; a directory is not a unit of anything.
2. **`lib.buri` is a library's whole public surface.** It exports, or
   re-exports, everything another target can see. A name that does not appear in
   `lib.buri` cannot be imported from outside the library, however public it is
   within it.
3. **`main.buri` is a compilation entry point** and exports `main`. Its build
   rule declares which outputs to produce — a Linux binary, a macOS binary, a JS
   module, or several at once.
4. **Tests live in `test/` and see only the target's surface.** A library's
   tests import `//lib/money`, not `//lib/money`'s internals. You test what
   dependents can call, so refactoring internals never rewrites a test. A
   library's fixtures *for other people's tests* live in `testing/`, and that
   segment in a path is what makes them unimportable from production code.
5. **Everything is declared.** Sources, test sources, dependencies, outputs,
   visibility, tags. No globs, no discovery, no implicit `..` walk. A file on
   disk that no rule lists is an error.

## A package, end to end

`lib/money/BUILD.buri`:

```textproto
library {
  name: "money"
  sources: [
    "cents.buri",
    "parse.buri",
  ]
  visibility: ["//visibility:public"]

  test {
    sources: [
      "test/cents.buri",
      "test/parse.buri",
    ]
  }
}
```

`lib.buri` is not in `sources` — the rule kind names it, the way `Library` names
it. Everything else in the package is listed one path at a time.

`lib/money/lib.buri`:

```buri
// The public surface of //lib/money. A dependent can import these names and no
// others; `toCents` below is exported by cents.buri but not from here, so it is
// visible inside this library and nowhere else.
from "//lib/money/cents" export { Cents, fromDollars, fromCents, add, isZero, format };
from "//lib/money/parse" export { ParseError, parse };
```

Module paths are absolute — there are no relative imports — so `//lib/money`
is the library's surface and `//lib/money/cents` is one module inside it, which
resolves only from within the library.

`lib/money/cents.buri`:

```buri
/// Money is never a raw integer. The field is not exported, so no caller can
/// add a Cents to an I64 by accident.
export struct Cents(I64);

export fn fromDollars(d: I64): Cents { Cents(d * 100) }
export fn fromCents(c: I64): Cents { Cents(c) }
export fn add(self: Cents, other: Cents): Cents { Cents(self.0 + other.0) }

// Exported from this module, so `parse.buri` can reach it. Not re-exported
// from lib.buri, so it is invisible outside //lib/money — as a free function
// and as a method.
export fn toCents(self: Cents): I64 { self.0 }
```

`lib/money/test/cents.buri`:

```buri
from "//lib/money" import { fromCents };
from "core/testing/assert" import * as assert;
from "core/testing/context" import { context };

test "pads the cents place" {
  let ctx = context();
  assert.eq(fromCents(1905).format(ctx), "\$19.05");
}

// assert.eq(fromCents(1905).toCents(), 1905)  // ERROR: `toCents` is not
//   exported by //lib/money. The test reaches the library the same way every
//   other dependent does.
```

And the binary that uses it, `cmd/server/BUILD.buri`:

```textproto
binary {
  name: "server"
  sources: ["routes.buri"]
  dependencies: [
    "//lib/ledger",
    "//lib/money",
    "//lib/store",
  ]
  tags: ["server"]

  outputs: [
    { platform: LINUX, arch: X86_64 },
    { platform: MACOS, arch: ARM64 },
  ]

  test {
    sources: ["test/routes.buri"]
  }
}
```

`buri build //cmd/server` produces two artifacts. `buri test //...` runs every
test in the repository. Neither command consults anything not named above.

## Tags, in one paragraph

`REPO.buri` declares the axes a build can vary along — `platform` is
predeclared, `tier` with values `client` and `server` is one you might add — and
declares how each axis composes along a dependency edge. `tags` mean the same
thing on every target, library and binary alike: the configurations it accepts.
`tags: ["server"]` means "only ever built with tier=server," and no tags means
"anywhere." A binary's configuration is then *resolved* — from its `outputs`,
its own tags, and every dependency's — so a `client` binary that reaches,
through four hops, a library tagged `server` fails at the dependency edge that
introduced it, with the path printed. Composition is per axis and declared:
`INTERSECT` for permissions that narrow, `PROPAGATE` for facts that spread
(licensing, maturity), `INDEPENDENT` for neither. [`TAGS.md`](./TAGS.md) has the
rules and the error messages.

## What the language buys the build system

The two designs lean on each other more than is usual, and the places where
they meet are the ones worth reviewing hardest:

| Language property | What the build system gets |
|---|---|
| Mandatory top-level signatures | A library's *interface* hash is derivable without compiling its bodies. Editing a private function does not invalidate a single dependent's typecheck. |
| Modules check independently | Compile actions within a package parallelize with no ordering constraints beyond the dep graph. |
| No macros, no reflection, no conditional compilation | A source file's meaning does not depend on the configuration it is built in, so a cache key is (sources, deps, flags) and nothing else. |
| Effects arrive as bounds on `ctx` | Hermeticity is not only a sandbox property. A test whose calls never passed a `Net`-bounded context cannot reach the network even if the sandbox leaks. |
| `Result` is must-use | A `Result` a test forgets to check does not compile, so a test cannot silently pass. |
| No relative module paths | A file's imports do not change when it moves, so `buri gen` can rewrite a build file without rewriting source. |
| No mutation, no global state | Test order is not observable; the runner may shard and reorder freely. |
| Circular imports are already an error | Package cycles are the same rule one level up, with the same diagnostic shape. |

## What is deliberately absent

- **No expression language in `BUILD.buri`.** No conditionals, no variables, no
  string concatenation, no macros or rule authoring. Build files are data. The
  cost is repetition, paid for by `buri gen` writing most of it.
- **No globs.** `sources: ["*.buri"]` is not accepted. A glob makes the file list
  depend on the state of the filesystem, which is precisely the input
  hermeticity is trying to pin down. `buri gen` enumerates for you.
- **No external repositories yet.** The `@repo//pkg` label syntax is reserved
  and unimplemented; today the only sources are your repository and `core/*`.
- **No user-defined rules or toolchains.** Two rule kinds, and the compiler is
  the only tool.
- **No remote cache or remote execution yet.** The cache key design in
  [`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md) is chosen so that
  adding one later is a transport change, not a semantic one.

## Settled, and what each one costs

These were the open questions of the first draft. They are decided; the costs
are recorded because a later change should have to argue with them.

1. **`test` and `assert` are reserved words** ([`SPEC.md`
   §11.2](../SPEC.md)). *Costs:* no function may be named `test`, no namespace
   `assert`. The alternatives were a naming convention (`export fn testFoo`),
   which the compiler cannot check, and an attribute syntax, which the language
   does not have and should not grow for one feature.
2. **One library per package.** *Buys:* `//lib/money` names a directory, a
   library, and a module all at once, so no label ever needs a `:target`.
   *Costs:* splitting a library in two is a directory move, which is real work
   in a large repository.
3. **`lib.buri` may hold logic.** It is an ordinary module that also
   re-exports. *Costs:* the surface is no longer trivially auditable by shape —
   a reader has to notice `export fn` alongside the re-exports. The stricter
   rule would have forced a one-line module for every small helper.
4. **Tags mean the same thing on a binary and on a library**, and how they
   compose is per-dimension and user-declared (`INTERSECT`, `PROPAGATE`,
   `INDEPENDENT` — [`TAGS.md`](./TAGS.md)). *Costs:* a binary's configuration is
   *resolved* rather than stated, so a dimension nothing narrows is an error
   that reads as "you did not choose" instead of "you must write this here".
5. **Golden files are updated by `buri test --accept`**, a separate
   non-hermetic mode that only ever rewrites files already declared in
   `test { data: ... }` ([`TESTING.md`](./TESTING.md)). *Costs:* a second code
   path through the runner, and a flag that can overwrite a fixture that was
   correct.
6. **Test-only code is marked by its path, not by a field.** Any module path
   containing a `testing` segment — `core/testing/assert`,
   `//lib/ledger/testing`, `//lib/testing/fakes` — is importable only from a
   test source ([`LIBRARIES.md`](./LIBRARIES.md#the-testing-surface)).
   *Costs:* `testing` is a reserved directory name, and the rule is a
   convention the compiler enforces rather than something a rule declares, so
   there is nothing to grep for in a build file.

## Still open

1. **External repositories.** The `@repo//pkg` label syntax is reserved and
   unimplemented; today the only sources are your repository and `core/*`.
2. **Remote caching and execution.** Not specified. The action-key design in
   [`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md) is chosen so
   that adding one is a transport change rather than a semantic one.
