---
name: buri-build
description: Use when adding or editing REPO.buri and BUILD.buri files, laying out packages and libraries, wiring dependencies, visibility, tags, or build outputs in a Buri repository.
---

# Buri: the build system

A monorepo build system. Build files are textproto data with no expression
language, and `buri gen` writes most of them. `buri docs build/overview`,
`build/build-files`, `build/libraries`, `build/tags`, `build/repo-config` are
the normative pages.

## Five rules the layout follows from

- **A directory with a `BUILD.buri` is a package.** Subdirectories without
  one belong to the nearest ancestor package.
- **`lib.buri` is a library's whole public surface.** Nothing outside the
  library can reach a name it does not export, as a function or as a method.
- **`main.buri` is a binary's entry point** and exports `main`. Its rule
  declares which outputs to produce.
- **Tests live in `test/` and see only the target's surface.** Fixtures for
  *other people's* tests live in `testing/`.
- **Everything is declared.** No globs, no discovery. A `.buri` file that no
  rule lists is an error, and one listed twice is an error too.

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
a rule has no `name` field.

In `dependencies` a label always means the *library* of that package. In a CLI
argument it means every target in it. Patterns are CLI-only: `//lib/...`,
`//...`. Labels are always repository-absolute.

## `REPO.buri`

A directory with a `REPO.buri` is a repository root. The file parses as
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

`lint` says where the lint catalogue runs, what a finding costs, and which
rules run. `check_during_build` makes `buri build` and `buri test` run it too.
`fail_on_finding` makes a finding fail whichever command reported it.
`rules { default: ENABLED|DISABLED, <lint_code>: bool }` turns rules off or on
by name — `enabled(rule) = override.unwrap_or(default)`, one field per lint code
with the hyphens underscored, so `default: DISABLED` plus a few `true` gives you
an allow list. Both booleans default to false, every rule defaults to on, and an
unknown rule name is `unknown-field`. A command whose repository turned rules
off prints which. `buri lint` exits nonzero on any finding, whatever this file
says.

Those two are the only fields: no `flags`, no toolchain pin, no `name`, no
defaults block, no per-directory or per-file lint exemption, no dependency
versions, no profiles, no environment.

## `BUILD.buri`

Textproto that parses as `buri.build.v1.BuildFile`. `#` starts a comment. No
variables, conditionals, concatenation, globs, `load` or rule authoring:
`sources: ["*.buri"]` is refused.

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
| `tags` | Labels saying what this code is. The policy lives in `REPO.buri`. |
| `platforms` | Omit unless the code is genuinely platform-specific. Unset means all. |
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

`main.buri` is required, and you leave it out of `sources` as you leave out
`lib.buri`. A `binary` takes **no `visibility`** and no `platforms`; `outputs`
says where it runs. Each output is a separate artifact and a separate check of
the whole graph, so a build can succeed for Linux and fail for JS. Name an
artifact with `artifact_name` on the output, not on the rule.

An empty rule is enough to start, and `gen` never invents one:

```textproto
library {}
```

## A package with both rules

The two `sources` sets are disjoint. The binary **implicitly depends on the
co-located library**, so do not list it. It reaches that library only through
`//tools/report`, never `//tools/report/render.buri`. The library cannot reach
the binary at all.

## Visibility

| Pattern | Matches |
|---|---|
| `//visibility:public` | anything |
| `//visibility:private` | only the same package |
| `//lib/...` | any package under `lib/`, including `lib` |
| `//lib/money` | that one package |

Leave `visibility` out and the target is private; there is no package or
repository default. Visibility applies to the **declared edge**, not
transitively. Two edges skip the check: a target's own suite reaching the target
under test, and a binary reaching the library in its own package.

## Dependencies

- **Use is what requires a dependency, and importing is not the only way to
  use.** A method resolves through its receiver's type, so calling
  `e.amount.format(ctx)` where `amount` is a `Cents` from `//lib/money` needs
  `//lib/money` in `dependencies`, even though no import names it.
- Dependencies are **direct**: a library you use is one you declare.
- `core/*` and `ui/*` ship with the toolchain, so never list them.
- **Every use needs an entry.** A use without one is an error (`missing-dep`),
  and `buri gen` adds it.
- Cycles are an error at the package level exactly as at the module level.

## Module paths

**You name a surface as a module. Everything else is a file, and only its own
package may name it.** The root is `//` for this repository, `core/` or `ui/`
for the standard library.

| Written | Is | Legal from |
|---|---|---|
| `"core/list"` | a standard library module | anywhere |
| `"//lib/money"` | the library's surface | where the dependency is declared and visibility granted, its own suite included |
| `"//lib/money/testing"` | the testing surface | only from a test source |
| `"//lib/money/cents.buri"` | one module inside it | only from inside `//lib/money` |
| `"//cmd/server/main.buri"` | a binary's entry point | only from that binary's own test sources |
| `"//proto/address.proto"` | a schema | as an ordinary module of its package |

`//lib/money` is a *label*, not a module path: it names a package in
`dependencies` and on the command line. Write one where the other belongs and
you get `import-path-without-a-file`.

`lib.buri` is made of re-exports, and may declare things itself:

```buri
from "//lib/money/cents.buri" export { Cents, fromCents, add, format };
from "//lib/money/parse.buri" export { ParseError, parse };
```

Export `add` and callers get both `add(a, b)` and `a.add(b)`. Leave `toCents`
out and they get neither (`not-on-the-surface`). A type's methods must live in
the module that declares the type.

## Tags and platforms

Tags are **labels saying what code is**, the same on a library and on a binary.
What follows from a tag is declared once, on the tag:

- `forbids { tags: [...] }` — two tags that forbid each other may not appear
  anywhere in the same dependency closure. It is symmetric, checked at every
  target, and a **union over the closure** rather than a path.
- `requires { platforms: [...] }` — a **whitelist**, never an exclusion.
  `platforms(T)` is the intersection over the closure, and an empty intersection
  is an error at the target itself (`unsatisfiable-target`).

The vocabulary is **closed**: a `tags` entry naming no `tag` block in
`REPO.buri` is an error (`unknown-tag`).

`Platform` is `LINUX`, `MACOS`, `JS`, `WEB`, and adding one is a compiler
change. A platform *is* the set of effects its host exports, so a `main` binding
`Ui: host.ui` under `platform: JS` fails with `effect-not-on-platform` as you
edit the file, on every output the binary declares.

There is no `#if` and no conditional compilation: two implementations means two
libraries with different `platforms` and one dependent that picks. Tags are not
visibility, and not a boolean expression language.

## Caching and hermeticity

An action's key is the toolchain version, the build mode, the platform, and the
content of every input, so the same commit hits the same entries on another
machine. **Tags never enter a cache key.** Actions run with an empty
environment. A file lock serializes cache writes, so any number of `buri`
processes can work in one repository at once. Two builds of one commit in one
configuration produce byte-identical artifacts, and
`buri build --check-reproducible` checks that.

If you reach for `buri clean` to fix a build, report it as a bug.

## Keeping build files right

```
buri gen //...            rewrite the fields that restate the sources
buri gen //... --check    exit 1 if anything would change; write nothing
buri format               canonical layout for sources and build files
buri lint //...           the graph rules: missing-dep, visibility, tags
```

`gen` rewrites exactly seven fields, sorted: `sources`, `proto_sources`,
`dependencies`, `test.sources`, `test.dependencies`, `testing.sources` and
`testing.dependencies`. It touches nothing else — rules, `tags`, `platforms`,
`visibility`, `outputs`, `timeout_seconds` and every comment survive — and it
never creates a build file.
