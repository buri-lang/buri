# The Buri build system

A monorepo build system for Buri: `BUILD.buri` files in textproto, one CLI that
builds, tests, lints, formats, and generates build files, hermetic actions, and
an incremental cache keyed on content rather than on timestamps.

**This is a design document, not an implementation**, in the same sense that
[`SPEC.md`](../SPEC.md) is. It is written to be specific enough to argue with.

| Document | What it covers |
|---|---|
| [`build-files.md`](./build-files.md) | Packages, labels, the `library` and `binary` rules, visibility |
| [`libraries.md`](./libraries.md) | `lib.buri` as the only public surface, re-exports, import resolution |
| [`tags.md`](./tags.md) | Build outputs, tags and the policy attached to them, platform restrictions |
| [`testing.md`](./testing.md) | The `test` declaration, the test platform, what a test can reach |
| [`repo-config.md`](./repo-config.md) | `REPO.buri`: the tag vocabulary, the lint policy, and what a repository-wide file deliberately does not hold |
| [`cli.md`](./cli.md) | `buri build`, `test`, `run`, `format`, `lint`, `gen`, `query` |
| [`hermeticity.md`](./hermeticity.md) | Sandboxing, action graph, cache keys, incrementality |
| [`schema/build.proto`](../schema/build.proto) | The normative schema for `BUILD.buri` |
| [`schema/repo.proto`](../schema/repo.proto) | The normative schema for `REPO.buri` |
| [`example/`](../../../tests/example/) | A complete worked monorepo — every snippet below is from it |

## The shape of a repository

```
REPO.buri                     # repository root, tag vocabulary, lint policy
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

```textproto schema=build
library {
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

```buri repo=cli/tests/example package=//lib/money
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

impl Cents {
  export fn add(self, other: Cents): Cents { Cents(self.0 + other.0) }

  // Exported from this module, so `parse.buri` can reach it. Not re-exported
  // from lib.buri, so it is invisible outside //lib/money — as a free function
  // and as a method.
  export fn toCents(self): I64 { self.0 }
}
```

`lib/money/test/cents.buri`:

```buri repo=cli/tests/example role=test
from "//lib/money" import { fromCents };
from "core/testing/assert" import * as assert;
from "core/testing/context" import { Hermetic };

test "pads the cents place" {
  let ctx = Hermetic();
  assert.eq(fromCents(1905).format(ctx), "\$19.05");
}
```

A suite is compiled with its own package, so it reaches its library's internals
the way any other file in the package does. The boundary is what a *dependent*
sees, and it is the compiler's rule rather than a convention — a name `lib.buri`
withholds is not callable from another package, as a free function or as a
method. `tools/report/test/render.buri`:

```buri repo=cli/tests/example role=test package=//tools/report
# from "//lib/money" import { fromCents };
# from "core/testing/assert" import * as assert;
test "the surface is the whole of what a dependent can call" {
  assert.eq(fromCents(1905).toCents(), 1905);   // ERROR: `toCents` is not on //lib/money's surface
}
```

And the binary that uses it, `cmd/server/BUILD.buri`:

```textproto schema=build
binary {
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

1. **`test` and `assert` are reserved words** ([`SPEC.md`
   §11.2](../SPEC.md)). *Costs:* no function may be named `test`, no namespace
   `assert`.
2. **One library per package.** *Buys:* `//lib/money` names a directory, a
   library, and a module at once, so no label ever needs a `:target`. *Costs:*
   splitting a library in two is a directory move.
3. **`lib.buri` may hold logic** ([`libraries.md`](./libraries.md#the-re-export-declaration)).
   *Costs:* the surface is no longer auditable by shape — a reader has to
   notice `export fn` alongside the re-exports.
4. **Tags are labels, and the policy lives on the tag declaration**
   ([`tags.md`](./tags.md)). *Costs:* nothing can require a binary to take a
   position, since a `forbids` rule fires only once two tags collide.
   Enforcement is opt-in, traded for there being no resolution algorithm.
5. **Golden files are updated by `buri test --accept`**
   ([`testing.md`](./testing.md#test-data-and-golden-files)). *Costs:* a second
   code path through the runner, and a flag that can overwrite a fixture that
   was correct.
6. **Test-only code is marked by its path, not by a field**
   ([`libraries.md`](./libraries.md#the-testing-surface)). *Costs:* `testing`
   is a reserved directory name, and there is nothing to grep for in a build
   file.

## Still open

1. **External repositories.** The `@repo//pkg` label syntax is reserved and
   unimplemented; today the only sources are your repository and `core/*`.
2. **Remote caching and execution.** Not specified. The action-key design in
   [`hermeticity.md`](./hermeticity.md) is chosen so
   that adding one is a transport change rather than a semantic one.
