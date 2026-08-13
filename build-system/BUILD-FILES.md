# `BUILD.buri`

The schema is [`schema/build.proto`](./schema/build.proto); this document is the
prose version, with the reasoning.

## The file

`BUILD.buri` is textproto parsing as `buri.build.v1.BuildFile`. It has no
expression language: no variables, no conditionals, no string concatenation, no
globs, no `load`, no rule authoring. Everything a rule depends on is written out
in the rule.

Textproto over a bespoke syntax because the schema is then a real artifact
rather than documentation — the parser rejects an unknown field with a line
number, an editor completes field names, and the CLI reads build files with the
same `.proto` machinery the language uses for wire formats.

```textproto
# lib/money/BUILD.buri

library {
  name: "money"
  srcs: [
    "cents.buri",
    "format.buri",
  ]
  visibility: ["//visibility:public"]

  test {
    srcs: [
      "test/cents.buri",
      "test/format.buri",
    ]
  }
}
```

`#` starts a comment. `buri fmt` formats build files as well as source: one
field per line, `srcs` and `deps` sorted, trailing commas, two-space indent.

## Packages

A directory containing a `BUILD.buri` is a **package**. Files in subdirectories
that do not contain their own `BUILD.buri` belong to the nearest ancestor
package.

```
lib/ledger/
  BUILD.buri         <- package boundary; everything below is //lib/ledger
  lib.buri
  entry.buri
  posting/
    rules.buri       <- src is "posting/rules.buri", still //lib/ledger
```

Subdirectories are for organizing a library that has grown, and cost nothing:
no rule, no visibility, no dependency edge. The only thing that creates a
boundary is a `BUILD.buri`.

A package declares **at most one library and at most one binary**. That falls
out of the fixed entry-point filenames — one `lib.buri` per directory, one
`main.buri` per directory — and it is what lets a dependency edge be written
`//lib/money` with no target name, since a dependency is always on a library.

## Labels

```
//lib/money            the library in package lib/money
//lib/money:money      the same target, written out
//cmd/server:server    a target by name — required for binaries
```

The short form `//pkg` resolves to the package's *library*. A binary is always
addressed by name, because `//cmd/server` in a `deps` list would otherwise read
as a dependency on something that cannot be depended on.

Patterns, accepted by the CLI and never in a build file:

```
//lib/money:all        every target in the package, including the binary
//lib/...              every target in the package and its subpackages
//...                  every target in the repository
```

Labels are always repository-absolute. There is no relative label form: a label
means the same thing wherever it is written, including in a CLI invocation from
a subdirectory.

## `library`

```textproto
library {
  name: "ledger"
  srcs: [
    "entry.buri",
    "posting/rules.buri",
  ]
  deps: ["//lib/money"]
  tags: ["server"]
  visibility: ["//cmd/...", "//lib/reporting"]

  test {
    srcs: ["test/ledger.buri"]
    deps: ["//lib/testing/fakes"]
    data: ["test/golden/ledger.txt"]
  }
}
```

| Field | Meaning |
|---|---|
| `name` | Target name. Required. Conventionally the directory name; `buri lint` warns when it is not. |
| `srcs` | Every `.buri` file in the package that belongs to this library, **excluding** `lib.buri` and the test sources. Package-relative, may descend into subdirectories. |
| `deps` | Labels of libraries this one may import. |
| `tags` | Where this library may be linked. See [`TAGS.md`](./TAGS.md). |
| `visibility` | Who may depend on it. Defaults below. |
| `test` | The test suite for this library. See [`TESTING.md`](./TESTING.md). |

`lib.buri` is required and is not listed in `srcs`. The rule kind names the
entry point the way `binary` names `main.buri`: it is not an input among
others, it is the thing the rule is *about*. Listing it would also make it
possible to write a `library` without one, which is not a state the build system
wants to have a diagnostic for.

Every other `.buri` file in the package must appear in exactly one rule's `srcs`
or `test.srcs`. A file that appears in none is an error —

```
error: lib/ledger/posting/interest.buri is not declared by any rule
  --> lib/ledger/BUILD.buri
   |
   = add it to library "ledger" srcs, or delete it
   = run `buri gen //lib/ledger` to do this automatically
```

— and a file that appears in two is an error as well. The alternative, ignoring
undeclared files, means a file can be deleted from the build by a typo in a
path and never noticed.

## `binary`

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
    { platform: LINUX, arch: ARM64 },
    { platform: MACOS, arch: ARM64 },
  ]

  test {
    srcs: ["test/routes.buri"]
  }
}
```

`main.buri` is required, is not listed in `srcs`, and must export `main` with
the signature [`SPEC.md` §11](../SPEC.md) requires. The context type `main`
declares is checked against the platform for each output: a `main` asking for
`fs: Fs` under `platform: JS` is a compile error at the entry point, not a
runtime failure in a browser.

`outputs` is a list because one entry point commonly ships several ways. Each
entry is a **separate configuration**, so the whole dependency graph is
tag-checked once per output, and `buri build //cmd/server` may succeed for Linux
and fail for JS. Address one with `buri build //cmd/server --output=js`.

A `binary` has no `visibility` field: nothing can depend on a binary. Use
`buri run` or `buri build`, and if two binaries need shared code, that code is a
library.

## A package with both

The user-visible case for this is a library that also ships a small CLI:

```
tools/report/
  BUILD.buri
  lib.buri            <- the library: rendering, testable
  render.buri
  main.buri           <- the binary: argv, stdout
  flags.buri
  test/
    render.buri       <- tests //tools/report
    flags.buri        <- tests //tools/report:report_cli
```

```textproto
# tools/report/BUILD.buri

library {
  name: "report"
  srcs: ["render.buri"]
  visibility: ["//visibility:public"]

  test {
    srcs: ["test/render.buri"]
  }
}

binary {
  name: "report_cli"
  srcs: ["flags.buri"]
  outputs: [{ platform: LINUX, arch: X86_64 }]

  test {
    srcs: ["test/flags.buri"]
  }
}
```

Two rules, one directory, one build file. The rules are:

- **The `srcs` sets are disjoint.** Every file belongs to exactly one rule.
- **The binary implicitly depends on the co-located library.** It does not
  appear in `deps`; a self-edge inside a package would be the only label in the
  system pointing at itself.
- **The binary reaches the library only through `./lib`.** `main.buri` may write
  `from "./lib" import { render };` and may not write `from "./render" import
  ...`. The library boundary is a property of the library, not of the directory,
  so it holds even for a file sitting next to it.
- **The library may not reach the binary at all.** `lib.buri` importing
  `./main` is an error.

## Visibility

`visibility` is a list of patterns. A dependency edge is legal when the
depending target's package matches at least one of them.

| Pattern | Matches |
|---|---|
| `//visibility:public` | Anything. |
| `//visibility:private` | Only targets in the same package. |
| `//lib/...` | Any package under `lib/`, including `lib` itself. |
| `//lib/money` | That one package. |
| `//cmd/server:server` | That one target. |

Resolution order for a rule that omits `visibility`: the package's
`package { default_visibility: ... }`, then `RepoConfig.defaults.visibility`,
then `//visibility:private`. Setting the repository default to `private` and
opening surfaces deliberately is the recommended posture, and it is what the
[`example/`](./example/) repository does.

```textproto
# lib/store/BUILD.buri — the database layer is not for general use

library {
  name: "store"
  srcs: ["file_store.buri", "codec.buri"]
  deps: ["//lib/ledger", "//lib/money"]
  tags: ["server"]
  visibility: ["//cmd/server:server", "//lib/store/..."]
}
```

The diagnostic names the rule that has to change, since that is where the
decision lives:

```
error: //cmd/web:web depends on //lib/store, which is not visible to it
  --> cmd/web/BUILD.buri:6:5
   |
 6 |     "//lib/store",
   |     ^^^^^^^^^^^^^
   |
   = //lib/store is visible to: //cmd/server:server, //lib/store/...
   = to allow this, add "//cmd/web:web" to visibility in lib/store/BUILD.buri
```

Visibility is checked on the **declared edge**, not transitively. If `//cmd/web`
depends on `//lib/ledger` and `//lib/ledger` depends on `//lib/store`, then
`//lib/store` needs to be visible to `//lib/ledger` and to nobody else.
Restricting what may travel through a transitive chain is what tags are for, and
they are the right tool for it because the constraint follows the code rather
than the edge.

## Dependencies

- `deps` lists **libraries only**. A binary is not a valid dependency.
- `core/*` is part of the toolchain and is never listed. It is available to
  every target, and the purity tiers in [`SPEC.md` §11.1](../SPEC.md) already
  govern what any given import of it can do.
- **Cycles are an error**, at the package level exactly as at the module level.
  The diagnostic prints the cycle in the order the edges were declared.
- **Every dep must be used, and every import must have a dep.** An import of
  `//lib/money` with no matching entry in `deps` is an error at the import; a
  `deps` entry that no source imports is an error at the build file. Both are
  errors and not warnings, because both make the dependency graph a description
  of something other than the code, and `buri gen` fixes either in one command.

```
error: cmd/server/routes.buri imports //lib/money, which is not in deps
  --> cmd/server/routes.buri:3:6
   |
 3 | from "//lib/money" import { Cents, format };
   |      ^^^^^^^^^^^^^
   |
   = add "//lib/money" to deps in cmd/server/BUILD.buri
   = run `buri gen //cmd/server` to do this automatically
```

## Generated build files

`buri gen //lib/money` rewrites `srcs`, `deps`, `test.srcs`, and `test.deps`
from what the sources actually contain, and touches nothing else. It requires
the `BUILD.buri` to already exist with the rule blocks and their `name` fields —
it never invents a target, because deciding that a directory should become a
library is a design decision and inferring it from the presence of a file is how
a repository ends up with two hundred libraries nobody chose. A stub is enough:

```textproto
library { name: "money" }
```

See [`CLI.md`](./CLI.md) for exactly which fields are managed and how comments
and hand-written fields survive the rewrite.
