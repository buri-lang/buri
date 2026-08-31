---
name: buri-build
description: Use when adding or editing REPO.buri and BUILD.buri files, laying out packages and libraries, wiring dependencies, visibility, tags, or build outputs in a Buri repository.
---

# Buri: the build system

A monorepo build system. Build files are textproto data with no expression
language; `buri gen` writes most of them. `buri docs build/overview`,
`build/build-files`, `build/libraries`, `build/tags`, `build/repo-config` are
the normative pages.

## Five rules the layout follows from

1. **A directory with a `BUILD.buri` is a package.** Subdirectories without
   one belong to the nearest ancestor package. A directory is not a unit of
   anything.
2. **`lib.buri` is a library's whole public surface.** A name it does not
   export is unreachable from outside the library — as a function *and* as a
   method.
3. **`main.buri` is a binary's entry point** and exports `main`. Its rule
   declares which outputs to produce.
4. **Tests live in `test/` and see only the target's surface.** Fixtures for
   *other people's* tests live in `testing/`.
5. **Everything is declared.** No globs, no discovery. A `.buri` file that no
   rule lists is an error; one listed twice is an error too.

```
REPO.buri                  # repository root, tag vocabulary, lint policy
lib/money/
  BUILD.buri               # declares //lib/money
  lib.buri                 # the entire public surface
  cents.buri               # internal
  testing/lib.buri         # //lib/money/testing, for other suites
  test/cents.buri          # this library's own suite
cmd/server/
  BUILD.buri               # declares //cmd/server
  main.buri                # exports main
  routes.buri
  test/routes.buri
```

## Labels

A label is a package path and **never carries a target name**: `//lib/money`,
`//cmd/server`. A package holds at most one library and at most one binary, so
the path plus the rule kind already identifies a target — which is also why a
rule has no `name` field.

In `dependencies` a label always means the *library* of that package. In a CLI
argument it means every target in it. Patterns are CLI-only: `//lib/...`,
`//...`. Labels are always repository-absolute.

## `REPO.buri`

Its presence is what makes a directory a repository root. It parses as
`buri.build.v1.RepoConfig` and has **two fields**:

```textproto
tag {
    name: "server"
    doc: "runs on infrastructure we operate"

    forbids { tags: ["client"] }

    requires { platforms: [LINUX, MACOS] }
}

tag {
    name: "client"
    doc: "ships to a user's machine or browser"
}

lint {
    check_during_build: true
    fail_on_finding: true
}
```

`lint` says where the lint catalogue runs and what a finding costs, and nothing
else: `check_during_build` makes `buri build` and `buri test` run it too,
`fail_on_finding` makes a finding fail whichever command reported it. Both
default to false, both only tighten, and there is no field that turns a check
off, exempts a directory or downgrades a finding. `buri lint` exits nonzero on
any finding whatever this file says.

There is no `flags`, no toolchain pin, no `name`, no defaults block, no per-rule
lint configuration, no dependency versions, no profiles, and no environment. A
repository-wide knob is a dialect; a knob goes on the command or on the rule.

## `BUILD.buri`

Textproto parsing as `buri.build.v1.BuildFile`. `#` starts a comment. No
variables, no conditionals, no concatenation, no globs, no `load`, no rule
authoring — `sources: ["*.buri"]` is refused on purpose.

```textproto
library {
    sources: [
        "cents.buri",
        "parse.buri",
    ]
    dependencies: ["//lib/money"]
    tags: ["server"]
    visibility: ["//cmd/...", "//lib/reporting"]

    testing {
        sources: ["testing/fixtures.buri"]
    }

    test {
        sources: ["test/cents.buri"]
        dependencies: ["//lib/testing/fakes"]
    }
}
```

| Field | Meaning |
|---|---|
| `sources` | Every `.buri` in the package belonging to this library, **excluding** `lib.buri` and the test sources. Package-relative, may descend. |
| `proto_sources` | Every `.proto` schema belonging to it; each becomes a module. |
| `dependencies` | Labels of libraries this one may use. Libraries only. |
| `tags` | Labels saying what this code is; the policy lives in `REPO.buri`. |
| `platforms` | Omit unless the code is genuinely platform-specific; unset means all. |
| `visibility` | Who may depend on it. Default is private. |
| `test` | The suite. See the `buri-testing` skill. |
| `testing` | Utilities for *other people's* tests, rooted at `testing/lib.buri`. |

```textproto
binary {
    sources: ["routes.buri"]
    dependencies: [
        "//lib/ledger",
        "//lib/money",
    ]
    tags: ["server"]

    outputs: [
        { platform: LINUX, arch: X86_64 },
        { platform: MACOS, arch: ARM64 },
        { platform: JS },
    ]

    test {
        sources: ["test/routes.buri"]
    }
}
```

`main.buri` is required and is not listed in `sources`, exactly as `lib.buri`
is not. A `binary` has **no `visibility`** — nothing can depend on a binary —
and no `platforms`, because `outputs` already says. Each output is a separate
artifact and a separate check of the whole graph, so a build may succeed for
Linux and fail for JS. Name an artifact with `artifact_name` on the output
that wants it, not on the rule.

An empty rule is enough to start; `gen` never invents one:

```textproto
library {}
```

## A package with both rules

The `sources` sets are disjoint. The binary **implicitly depends on the
co-located library** — do not list it — and reaches it only through
`//tools/report/lib.buri`, never `//tools/report/render.buri`. The library may
not reach the binary at all.

## Visibility

| Pattern | Matches |
|---|---|
| `//visibility:public` | anything |
| `//visibility:private` | only the same package |
| `//lib/...` | any package under `lib/`, including `lib` |
| `//lib/money` | that one package |

Omitting `visibility` means private. There is no package or repository
default. Visibility is checked on the **declared edge**, not transitively. Two
edges skip the check: a target's own suite reaching the target under test, and
a binary reaching the library in its own package.

## Dependencies

- **Use is what requires a dep, and an import is not the only way to use.** A
  method resolves through its receiver's type, so calling `e.amount.format(ctx)`
  where `amount` is a `Cents` from `//lib/money` requires `//lib/money` in
  `dependencies` even though no import names it.
- Dependencies are **direct**: a library you use is one you declare.
- `core/*` and `ui/*` ship with the toolchain and are never listed.
- **Every use must have an entry** — a use with none is an error
  (`missing-dep`), and `buri gen` adds it.
- Cycles are an error at the package level exactly as at the module level.

## Module paths

**A module path names a file.** It is a root — `//` for this repository,
`core/` or `ui/` for the standard library — and then the path of a file under
it, extension and all.

| Written | Is | Legal from |
|---|---|---|
| `"core/list/lib.buri"` | a standard library module | anywhere |
| `"//lib/money/lib.buri"` | the library's surface | where the dependency is declared and visibility granted |
| `"//lib/money/cents.buri"` | one module inside it | only from inside `//lib/money` |
| `"//cmd/server/main.buri"` | a binary's entry point | only from that binary's own test sources |
| `"//lib/money/testing/lib.buri"` | the testing surface | only from a test source |
| `"//proto/address.proto"` | a schema | as an ordinary module of its package |

`//lib/money` is a *label* — it names a package in `dependencies` and on the
command line — and is not a module path. Writing one where the other belongs is
`import-path-without-a-file`, and inside a repository the diagnostic names the
file the old spelling meant.

`lib.buri` is made of re-exports and may also declare things itself:

```buri
from "//lib/money/cents.buri" export { Cents, fromCents, add, format };
from "//lib/money/parse.buri" export { ParseError, parse };
```

Exporting `add` makes both `add(a, b)` and `a.add(b)` available outside;
leaving out `toCents` removes both (`not-on-the-surface`). A type's methods
must be declared in the module that declares the type, so a library's file
layout follows its types, not its verbs.

## Tags and platforms

Tags are **labels saying what code is**, and mean the same thing on a library
and on a binary. What follows from a tag is declared once, on the tag:

- `forbids { tags: [...] }` — two tags that forbid each other may not appear
  anywhere in the same dependency closure. Symmetric, checked at every target,
  a **union over the closure** rather than a path.
- `requires { platforms: [...] }` — a **whitelist**, never an exclusion, so
  adding a platform to the toolchain cannot silently widen old code.
  `platforms(T)` is the intersection over the closure; an empty intersection
  is an error at the target itself (`unsatisfiable-target`).

The vocabulary is **closed**: a `tags` entry naming no `tag` block in
`REPO.buri` is an error (`unknown-tag`), not a harmless annotation.

`Platform` is `LINUX`, `MACOS`, `JS`, `WEB`; adding one is a compiler change.
A platform *is* the set of effects its host exports, so a `main` binding
`Net: host.net` under `platform: WEB` fails with `host-not-granted`.

Tags are **not** a boolean expression language, **not** conditional compilation
(there is no `#if`; two implementations means two libraries with different
`platforms` and one dependent that picks), and **not** a substitute for
visibility.

## Caching and hermeticity

An action is keyed on the toolchain version, the build mode, the platform, and
the content of every input — **tags never enter a cache key**. Content
addressing means moving the checkout or building the same commit on another
machine hits the same entries. Actions run with an empty environment. Cache
writes are serialized by a file lock and reads take none, so any number of
`buri` processes can work in one repository at once. Two builds of one commit
in one configuration produce byte-identical artifacts;
`buri build --check-reproducible` asks that of the repository.

Reaching for `buri clean` to fix a build is worth reporting as a bug.

## Keeping build files right

```
buri gen //...            rewrite the fields that restate the sources
buri gen //... --check    exit 1 if anything would change; write nothing
buri format               canonical layout for sources and build files
buri lint //...           the graph rules: missing-dep, visibility, tags
```

`gen` rewrites exactly seven fields — `sources`, `proto_sources`,
`dependencies`, `test.sources`, `test.dependencies`, `testing.sources`,
`testing.dependencies` — sorted, and touches nothing else: rules, `tags`,
`platforms`, `visibility`, `outputs`, `timeout_seconds` and every
comment survive. It never creates a build file.
