---
name: buri-build
description: Use when adding or editing REPO.buri and BUILD.buri files, laying out packages and libraries, wiring dependencies, visibility, tags, or build outputs in a Buri repository.
---

# Buri: the build system

Build files are textproto with no expression language, and `buri gen` writes
most of them. The normative pages are `buri docs build/overview`,
`build/build-files`, `build/libraries`, `build/tags` and `build/repo-config`.

## Layout rules

- **A directory with a `BUILD.buri` is a package.** Subdirectories without one
  belong to the nearest ancestor package.
- **`lib.buri` is a library's whole public surface.** Outside the library, a
  name it doesn't export is unreachable, as a function or as a method.
- **`main.buri` is a binary's entry point** and exports `main`.
- **Tests live in `test/` and see only the target's surface.** Fixtures for
  *other people's* tests live in `testing/`.
- **Everything is declared.** No globs, no discovery. A `.buri` file no rule
  lists is an error, and so is one listed twice.

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
tool/lines/
  BUILD.buri               # declares //tool/lines; every tool rule lives under tool/
  tool.buri                # exports check, format or generate
```

## Labels

A label is a repository-absolute package path and **never carries a target
name**: `//lib/money`, `//cmd/server`. A package holds at most one library, one
binary and one tool, so a rule has no `name` field.

In `dependencies` a label means the package's *library*. On the command line it
means every target in it. Patterns like `//lib/...` and `//...` are CLI-only.

## `REPO.buri`

A directory with a `REPO.buri` is a repository root. The file parses as
`buri.build.v1.RepoConfig` and has **three fields**:

```textproto
tag {
    name: "server"
    doc: "runs on infrastructure we operate"

    forbids { tags: ["client"] }

    requires { backends: [NATIVE] }
}

tag {
    name: "client"
    doc: "ships to a user's machine or browser"
}

lint {
    check_during_build: true
    fail_on_finding: true
}

language {
    name: "jsonc"
    extensions: [".code-workspace"]
}
```

`language` only adds extensions to a built-in language (`json`, `jsonc`,
`json5`, `proto`, `textproto`).

`lint` controls the lint catalogue:

- `check_during_build` (default false) makes `buri build` and `buri test` lint
  too.
- `fail_on_finding` (default false) makes a finding fail the command.
- `rules { default: ENABLED|DISABLED, <lint_code>: bool }` toggles rules by name,
  hyphens underscored. Every rule defaults to on; `default: DISABLED` plus a few
  `true` is an allow list. An unknown name is `build-unknown-field`. Commands print
  which rules are off.
- `buri lint` exits nonzero on any finding regardless.

Nothing else exists: no `flags`, toolchain pin, `name`, defaults, per-file lint
exemptions, dependency versions, profiles or environment.

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
| `generators` | Tools whose `generate` writes modules of this library. Each entry names a `tool` rule by `//label` or a built-in tool by bare name (`json`, `proto`, `textproto`), plus its `inputs`. A `.proto` schema goes here, under `proto`. |
| `dependencies` | Libraries this one may use. Libraries only. |
| `tags` | What this code is. The policy lives in `REPO.buri`. |
| `backends` | `NATIVE` or `JS`. Unset means both; set it only if the code relies on one. |
| `platforms` | `"native"`, `"node"`, `"web"` or a `"//platform/..."` label. Unset means all; set it only if the code means something on one platform only. |
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
        { platform: "native", variant: "linux-x86_64" },
        { platform: "native", variant: "macos-arm64" },
        { platform: "node" },
    ]

    test {
        sources: ["test/routes.buri"]
    }
}
```

- `main.buri` is required and, like `lib.buri`, stays out of `sources`.
- A `binary` takes **no `visibility`** and no `platforms`; `outputs` says where
  it runs. Each output is a separate artifact and a separate check of the whole
  graph, so a build can pass for `native` and fail for `node`.
- `artifact_name` goes on the output, not the rule.
- A `native` output names its `variant`: `linux-arm64`, `linux-x86_64`,
  `macos-arm64` or `macos-x86_64`.
- The function named `main` fills each platform's `main` entry;
  `entries: [{ name: "main", function: "other" }]` picks another.
- `platform: LINUX`, `platforms: [JS]`, and the output fields `arch`, `entry`
  and `js {}` are `retired-platform`.

An empty rule is enough to start, and `gen` never invents one:

```textproto
library {}
```

## A package with both rules

The two `sources` sets are disjoint. The binary **implicitly depends on the
co-located library**, so don't list it, and reaches it only through its surface
(`//tools/report`, never `//tools/report/render.buri`). The library can't reach
the binary.

## Visibility

| Pattern | Matches |
|---|---|
| `//visibility:public` | anything |
| `//visibility:private` | only the same package |
| `//lib/...` | any package under `lib/`, including `lib` |
| `//lib/money` | that one package |

Leave `visibility` out and the target is private; there is no package or
repository default. Visibility applies to the **declared edge**, not
transitively. Two edges skip the check: a target's own suite reaching the
target, and a binary reaching the library in its own package.

## Dependencies

- **Use requires a dependency, and importing isn't the only use.** A method
  resolves through its receiver's type, so `e.amount.format(ctx)`, where
  `amount` is a `Cents` from `//lib/money`, needs `//lib/money` in
  `dependencies` even though no import names it.
- Dependencies are **direct**: declare every library you use. A missing one is
  `missing-dependency`, and `buri gen` adds it.
- `core/*` and `ui/*` ship with the toolchain; never list them.
- Package cycles are an error, as module cycles are.

## Module paths

**You import a surface as a module. Any other file is reachable only from its
own package.** The root is `//` for this repository, `core/` or `ui/` for the
standard library.

| Written | Is | Legal from |
|---|---|---|
| `"core/list"` | a standard library module | anywhere |
| `"//lib/money"` | the library's surface | where the dependency is declared and visibility granted, its own suite included |
| `"//lib/money/testing"` | the testing surface | only from a test source |
| `"//lib/money/cents.buri"` | one module inside it | only from inside `//lib/money` |
| `"//cmd/server/main.buri"` | a binary's entry point | only from that binary's own test sources |
| `"//proto/address.proto"` | a schema | as an ordinary module of its package |

Mixing up a label and a module path is `import-missing-extension`.

`lib.buri` is made of re-exports, and may declare things itself:

```buri
from "//lib/money/cents.buri" export { Cents, fromCents, add, format };
from "//lib/money/parse.buri" export { ParseError, parse };
```

Export `add` and callers get both `add(a, b)` and `a.add(b)`. Leave `toCents`
out and they get neither (`not-on-surface`). A type's methods must live in
the module that declares the type.

## Tags and platforms

Tags **say what code is**. Their consequences are declared once, on the tag:

- `forbids { tags: [...] }`: two tags that forbid each other may not appear
  anywhere in the same dependency closure. It's symmetric, checked at every
  target, and a **union over the closure**, not a path.
- `forbids { backends: [...], platforms: [...] }`: what the tagged code may not
  be built or tested for. A platform added later is admitted.
- `requires { backends: [...], platforms: [...] }`: a **whitelist**. A platform
  added later isn't admitted until listed.
- A tag admits what its `requires` admits (all, when unset) minus what its
  `forbids` names. A target's platforms are the intersection over its closure,
  and an empty intersection is `unsatisfiable-target`.

The vocabulary is **closed**: a `tags` entry with no `tag` block in `REPO.buri`
is `unknown-tag`.

A repository adds its own platforms under `//platform/`
(`buri docs guides/custom-platforms`). Each entry takes its platform's host, whose
fields are the effects it offers: `main(host: NodeHost)` binding `Ui: host.ui`
is `unknown-field`. Platforms needing different code get different entries:
`entries: [{ name: "main", function: "mainForNode" }]`.

There's no `#if`: two implementations means two libraries with different
`backends` or `platforms` and one dependent that picks. Tags aren't visibility
or a boolean expression language.

## Caching and hermeticity

- An action's key hashes the `buri` binary, build mode, platform and input
  contents, so the cache is portable across machines. **Tags never enter a
  key.**
- Actions run with an empty environment, and output is byte-identical;
  `buri build --check-reproducible` checks it.
- Any number of `buri` processes can share a repository.

If you reach for `buri clean` to fix a build, report it as a bug.

## Keeping build files right

```
buri gen //...            rewrite the fields that restate the sources
buri gen //... --check    exit 1 if anything would change; write nothing
buri format               canonical layout for sources and build files
buri lint //...           the graph rules: missing-dependency, visibility, tags
```

`gen` rewrites exactly six fields, sorted: `sources`, `dependencies`,
`test.sources`, `test.dependencies`, `testing.sources` and
`testing.dependencies`. Everything else survives, comments included, and it
never creates a build file.
