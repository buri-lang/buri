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
   dependents can call, so refactoring internals never rewrites a test.
5. **Everything is declared.** Sources, test sources, dependencies, outputs,
   visibility, tags. No globs, no discovery, no implicit `..` walk. A file on
   disk that no rule lists is an error.

## A package, end to end

`lib/money/BUILD.buri`:

```textproto
library {
  name: "money"
  srcs: [
    "cents.buri",
    "parse.buri",
  ]
  visibility: ["//visibility:public"]

  test {
    srcs: [
      "test/cents.buri",
      "test/parse.buri",
    ]
  }
}
```

`lib.buri` is not in `srcs` — the rule kind names it, the way `Library` names
it. Everything else in the package is listed one path at a time.

`lib/money/lib.buri`:

```buri
// The public surface of //lib/money. A dependent can import these names and no
// others; `toCents` below is exported from ./cents but not from here, so it is
// visible inside this library and nowhere else.
from "./cents" export { Cents, fromDollars, fromCents, add, isZero, format };
from "./parse" export { ParseError, parse };
```

`lib/money/cents.buri`:

```buri
/// Money is never a raw integer. The representation is not exported, so no
/// caller can add a Cents to an I64 by accident.
export opaque struct Cents(I64);

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
from "core/cap" import { Alloc };
from "core/test" import * as t;
from "core/test" import { Expect };

test "pads the cents place" (ctx: { alloc: Alloc, expect: Expect }): Result<{}, Str> {
  t.eq(ctx, fromCents(1905).format(ctx), "\$19.05")?;
  .Ok({})
}

// t.eq(ctx, fromCents(1905).toCents(), 1905)  // ERROR: `toCents` is not
//   exported by //lib/money. The test reaches the library the same way every
//   other dependent does.
```

And the binary that uses it, `cmd/server/BUILD.buri`:

```textproto
binary {
  name: "server"
  srcs: ["routes.buri"]
  deps: [
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
    srcs: ["test/routes.buri"]
  }
}
```

`buri build //cmd/server` produces two artifacts. `buri test //...` runs every
test in the repository. Neither command consults anything not named above.

## Tags, in one paragraph

`REPO.buri` declares the axes a build can vary along — `platform` is
predeclared, `tier` with values `client` and `server` is one you might add. A
binary's configuration assigns exactly one value per axis, partly from its
`outputs` and partly from its `tags`. A library's `tags` are *constraints*:
`tags: ["server"]` means "only ever linked into a server binary," and a library
with no tags fits anywhere. Constraints compose up the graph — a library
inherits the intersection of its dependencies' constraints — so a `client`
binary that reaches, through four hops, a library tagged `server` fails at the
dependency edge that introduced it, with the path printed.
[`TAGS.md`](./TAGS.md) has the rules and the error messages.

## What the language buys the build system

The two designs lean on each other more than is usual, and the places where
they meet are the ones worth reviewing hardest:

| Language property | What the build system gets |
|---|---|
| Mandatory top-level signatures | A library's *interface* hash is derivable without compiling its bodies. Editing a private function does not invalidate a single dependent's typecheck. |
| Modules check independently | Compile actions within a package parallelize with no ordering constraints beyond the dep graph. |
| No macros, no reflection, no conditional compilation | A source file's meaning does not depend on the configuration it is built in, so a cache key is (sources, deps, flags) and nothing else. |
| Effects are values passed to `main` | Hermeticity is not only a sandbox property. A test that was never handed `Net` cannot reach the network even if the sandbox leaks. |
| `Result` is must-use | An assertion whose result you forget to check does not compile, so a test cannot silently pass. |
| No mutation, no global state | Test order is not observable; the runner may shard and reorder freely. |
| Circular imports are already an error | Package cycles are the same rule one level up, with the same diagnostic shape. |

## What is deliberately absent

- **No expression language in `BUILD.buri`.** No conditionals, no variables, no
  string concatenation, no macros or rule authoring. Build files are data. The
  cost is repetition, paid for by `buri gen` writing most of it.
- **No globs.** `srcs: ["*.buri"]` is not accepted. A glob makes the file list
  depend on the state of the filesystem, which is precisely the input
  hermeticity is trying to pin down. `buri gen` enumerates for you.
- **No external repositories yet.** The `@repo//pkg` label syntax is reserved
  and unimplemented; today the only sources are your repository and `core/*`.
- **No user-defined rules or toolchains.** Two rule kinds, and the compiler is
  the only tool.
- **No remote cache or remote execution yet.** The cache key design in
  [`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md) is chosen so that
  adding one later is a transport change, not a semantic one.

## Open questions

These want real repositories before they are settled.

1. **`test` as a reserved word.** [`TESTING.md`](./TESTING.md) adds a `test`
   declaration to the grammar, which costs `test` as an identifier — a function
   named `test` becomes illegal. The alternatives were a naming convention
   (`export fn testFoo`), which is not checkable, and an attribute syntax, which
   the language does not have and should not grow for one feature.
2. **One library per package.** It is what makes `//lib/money` name both a
   directory and a target, and what makes `deps` never need a `:name`. It also
   means splitting a library in two is a directory move, which is a real cost in
   a large repository.
3. **Whether `lib.buri` may hold logic.** Today it may — it is an ordinary
   module that also re-exports. A stricter rule ("re-exports only") would make
   the surface trivially auditable at the cost of forcing a one-line module for
   every small helper.
4. **Tag composition on binaries.** A library's constraints intersect. A
   binary's tags are assignments. Whether a binary should be able to *constrain*
   as well — "this binary must not transitively reach anything tagged
   `experimental`" — is unresolved; it is a different operation wearing the same
   word.
5. **Test data and hermeticity.** `test { data: [...] }` exposes files through
   an in-memory `Fs`. Golden-file tests want to *rewrite* those files, which no
   hermetic action may do. A `buri test --accept` escape hatch that runs outside
   the sandbox is the obvious answer and is not yet specified.
