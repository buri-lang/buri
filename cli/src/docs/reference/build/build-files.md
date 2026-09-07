# `BUILD.buri`

The schema is [`schema/build.proto`](../schema/build.proto). This page is the
prose version.

## The file

`BUILD.buri` is textproto that parses as `buri.build.v1.BuildFile`. There is no
expression language: no variables, no conditionals, no string concatenation, no
globs, no `load`, no rule authoring. Every rule writes out everything it depends
on, and `buri gen` writes most of the file for you. `sources: ["*.buri"]` is
rejected outright, because a glob makes the file list depend on the state of the
filesystem.

```textproto schema=build
# lib/money/BUILD.buri
library {
    sources: ["cents.buri", "parse.buri"]
    visibility: ["//visibility:public"]

    test {
        sources: ["test/cents.buri", "test/parse.buri"]
    }
}
```

`#` starts a comment. `buri format` formats build files as well as source: the
schema's field order, one field per line, trailing commas, four-space indent,
and every list left in the order you wrote it. `buri docs cli format` has the
whole of it. `buri docs cli gen` covers sorting, which belongs to `gen` because
`gen` decides what a managed list contains.

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
    rules.buri       <- source is "posting/rules.buri", still //lib/ledger
```

Subdirectories organize a library that has grown, and they cost nothing: no
rule, no visibility, no dependency edge. Only a `BUILD.buri` creates a boundary.

A package declares **at most one library and at most one binary**. The fixed
entry-point filenames force that: one `lib.buri` per directory, one `main.buri`
per directory. It is also what lets a label be a bare path.

## Labels

A label is a package path. It never carries a target name:

```
//lib/money        the library in package lib/money
//cmd/server       the package cmd/server
//lib              the package lib, if one has a BUILD.buri there
```

In a `dependencies` list a label means **the library** of that package, since a
library is the only thing you can depend on. In a CLI argument it means **every
target** in that package. So there is no `:name` syntax to learn.

For the same reason **a rule has no `name` field**. `//lib/money` is the
library, `//cmd/server` is the binary, and diagnostics print exactly that. One
thing does need a filename: the artifact a binary produces. That defaults to the
package's directory name, and you override it on the output that wants it:

```textproto ignore why="a fragment of a build file, not a whole one"
outputs: [
    { platform: LINUX, arch: X86_64, artifact_name: "report-cli" },
]
```

Patterns, accepted by the CLI and never in a build file:

```
//lib/...              every target in that package and its subpackages
//...                  every target in the repository
```

Labels are always repository-absolute. There is no relative form, so a label
means the same thing wherever you write it, including in a CLI invocation from a
subdirectory.

A label is also the module path an import writes for that library's surface.
`from "//lib/money" import …` resolves to its `lib.buri`, from inside the
package and from outside it alike. A label cannot name one file among the many a
package holds. That takes a path with a file name on it, and only that package
may write one. See [`libraries.md`](./libraries.md#module-paths).

## `library`

```textproto schema=build
library {
    sources: ["entry.buri", "posting/rules.buri"]
    dependencies: ["//lib/money"]
    tags: ["server"]
    visibility: ["//cmd/...", "//lib/reporting"]

    test {
        sources: ["test/ledger.buri"]
        dependencies: ["//lib/testing/fakes"]
    }
}
```

| Field | Meaning |
|---|---|
| `sources` | Every `.buri` file in the package that belongs to this library, **excluding** `lib.buri` and the test sources. Package-relative, may descend into subdirectories. |
| `proto_sources` | Every `.proto` schema in the package that belongs to this library. Each becomes a module named by its own path, holding the types it declares and their codecs. See [`proto.md`](./proto.md). |
| `dependencies` | Labels of libraries this one may use. |
| `tags` | Labels saying what this code is. `REPO.buri` declares the policy they carry. See [`tags.md`](./tags.md). |
| `platforms` | The platforms it can build for. Omit unless the code is genuinely platform-specific; unset means all of them. |
| `visibility` | Who may depend on it. Defaults below. |
| `test` | The test suite for this library. See [`testing.md`](./testing.md). |
| `testing` | The library's utilities *for other people's tests*, rooted at `testing/lib.buri`. See below. |

`lib.buri` is required, and `sources` does not list it. The rule kind names the
entry point, the way `binary` names `main.buri`.

Every other `.buri` file in the package must appear in exactly one rule's
`sources`, `test.sources`, or `testing.sources`, and every `.proto` in exactly
one rule's `proto_sources`. A file that appears in none, or in two, is an error:

```
error: lib/ledger/posting/interest.buri is not declared by any rule
  --> lib/ledger/BUILD.buri
   |
   = add it to the library's sources, or delete it
   = run `buri gen //lib/ledger` to do this automatically
```

### The `testing` block

A library's utilities for *other people's* tests live in a `testing/`
subdirectory with its own entry point. A `testing` block declares them:

```textproto schema=build
library {
    sources: ["entry.buri", "posting/rules.buri"]
    dependencies: ["//lib/money"]
    visibility: ["//cmd/...", "//lib/store", "//tools/report"]

    test {
        sources: ["test/ledger.buri"]
    }

    testing {
        sources: ["testing/fixtures.buri"]
    }
}
```

`testing/lib.buri` is required when the block is present, and `sources` does not
list it. The block is required when the file exists. It may be empty
(`testing {}`) when the entry point is the whole of it.
[`libraries.md`](./libraries.md#the-testing-surface) covers the surface it
declares and what those modules may import.

## `binary`

```textproto schema=build
binary {
    sources: ["routes.buri"]
    dependencies: ["//lib/ledger", "//lib/money", "//lib/store"]
    tags: ["server"]

    outputs: [
        { platform: LINUX, arch: X86_64 },
        { platform: LINUX, arch: ARM64 },
        { platform: MACOS, arch: ARM64 },
    ]

    test {
        sources: ["test/routes.buri"]
    }
}
```

`main.buri` is required, and `sources` does not list it. It must export `main`
with the signature [`language/programs.md` §11](../../language/programs.md)
requires: no parameters, returning `Result<(), Str>`. It is also the only module
in the binary that may import `core/host`.

A platform *is* the set of effects its host exports. A platform that does not
grant an effect does not export the name for it, so asking for it fails to
compile at the line that asked, as `effect-not-on-platform`. A `main` binding
`Ui: host.ui` under `platform: JS` does not compile, and neither does one
binding `FsRead: host.fs` under `platform: WEB`.
`buri docs error effect-not-on-platform` has the table of what each platform
grants.

The check does not wait for a build. The compiler checks `main.buri` against
**every** platform its `outputs` name, plus every platform its suite names in
`test.platforms`, since a test binary links `main` in. So a binary declaring
`[MACOS, WEB]` and binding `FsRead: host.fs` is refused whichever output you
ask for, and `buri lint`, `buri test` and the language server all refuse it
before anything is produced. Every other module is checked against the platforms
**its own rule declared**, and a rule that declared none is never checked.

`outputs` is a list because one entry point commonly ships several ways. The
compiler checks the whole dependency graph against each entry separately, so
`buri build //cmd/server` may succeed for Linux and fail for JS. Build one with
`buri build //cmd/server --output=js`. A binary has no `platforms` field of its
own, because `outputs` already says.

`tags` on a binary mean what they mean on a library. The tag check does not vary
across outputs, so it runs once no matter how many artifacts the binary
produces.

A `binary` has no `visibility` field, because nothing can depend on a binary.
Use `buri run` or `buri build`. When two binaries need shared code, that code is
a library.

## A package with both

The case for this is a library that also ships a small CLI:

```
tools/report/
  BUILD.buri
  lib.buri            <- the library: rendering, testable
  render.buri
  main.buri           <- the binary: command-line arguments, stdout
  flags.buri
  test/
    render.buri       <- tests //tools/report
    flags.buri        <- tests //tools/report/main
```

```textproto schema=build
# tools/report/BUILD.buri
library {
    sources: ["render.buri"]
    visibility: ["//visibility:public"]

    test {
        sources: ["test/render.buri"]
    }
}

binary {
    sources: ["flags.buri"]
    tags: ["server"]
    outputs: [
        { platform: LINUX, arch: X86_64 },
    ]

    test {
        sources: ["test/flags.buri"]
    }
}
```

The rule kind tells the two apart, and the binary's artifact is `report`, after
the directory. The rules:

- **The `sources` sets are disjoint.** Every file belongs to exactly one rule.
- **The binary implicitly depends on the co-located library.** You do not list
  it in `dependencies`.
- **The binary reaches the library only through `//tools/report`.** `main.buri`
  may import the library's surface, and may not import
  `//tools/report/render.buri`. The boundary belongs to the library rather than
  the directory, so it holds even for a file sitting next to it.
- **The library may not reach the binary at all.** `lib.buri` importing
  `//tools/report/main.buri` is an error.

## Visibility

`visibility` is a list of patterns. A dependency edge is legal when the
depending target's package matches at least one of them.

| Pattern | Matches |
|---|---|
| `//visibility:public` | Anything. |
| `//visibility:private` | Only targets in the same package. |
| `//lib/...` | Any package under `lib/`, including `lib` itself. |
| `//lib/money` | That one package. |

A rule that omits `visibility` is `//visibility:private`. There is no package
default and no repository default: the library's own rule is the one place that
decides who may depend on it, so an absent `visibility` never sends you to
another file.

```textproto schema=build
# lib/store/BUILD.buri — the database layer is not for general use
library {
    sources: ["codec.buri", "file_store.buri"]
    dependencies: ["//lib/ledger", "//lib/money"]
    tags: ["server"]
    visibility: ["//cmd/server"]
}
```

The diagnostic names the rule that has to change, because that is where the
decision lives:

```
error: //cmd/web depends on //lib/store, which is not visible to it
  --> cmd/web/BUILD.buri:6:5
   |
 6 |     "//lib/store",
   |     ^^^^^^^^^^^^^
   |
   = //lib/store is visible to: //cmd/server
   = to allow this, add "//cmd/web" to visibility in lib/store/BUILD.buri
```

Two edges skip the check, because nobody chose either one: a target's own test
suite reaching the target under test, and a binary reaching the library in its
own package. Everything else goes through the check, including a test suite
reaching a library named in `test.dependencies`.

The compiler checks visibility on the **declared edge**, not transitively. If
`//cmd/web` depends on `//lib/ledger` and `//lib/ledger` depends on
`//lib/store`, then `//lib/store` needs to be visible to `//lib/ledger` and to
nobody else. To restrict what travels through a transitive chain, use tags: a
tag follows the code rather than the edge, and the check runs over the whole
closure.

## Dependencies

- `dependencies` lists **libraries only**. A binary is not a valid dependency.
- **Use is what requires a dependency, and an import is not the only way to
  use.** A method resolves through its receiver's type rather than through
  scope, so

  ```buri repo=cli/tests/example
  # from "core/effect" import { Alloc };
  from "//lib/ledger" import { Entry };

  // `amount` is a Cents from //lib/money, and `format` is one of its methods —
  // no import names //lib/money, and this target still depends on it.
  fn line<C: Alloc>(ctx: C, e: Entry): Str {
      e.amount.format(ctx)
  }
  ```

  needs `//lib/money` in `dependencies` as much as an import would.
  Dependencies are direct. Declare every library you use, whether or not
  something else in the graph also pulls it in.
- `core/*` ships with the toolchain, and you never list it. Every target can use
  it, and the purity tiers in
  [`language/programs.md` §11.1](../../language/programs.md) already govern
  what any given import of it can do.
- **Cycles are an error**, at the package level exactly as at the module level.
  The diagnostic prints the cycle in the order you declared the edges.
- **Every entry must be used, and every use must have an entry.** Using
  `//lib/money` with nothing matching in `dependencies` is an error at the use
  site. A `dependencies` entry no source uses is an error at the build file.
  `buri gen` fixes both in one command.

```
error: cmd/server/routes.buri imports //lib/money, which is not in dependencies
  --> cmd/server/routes.buri:3:6
   |
 3 | from "//lib/money" import { Cents, format };
   |      ^^^^^^^^^^^^^
   |
   = fix: add "//lib/money" to dependencies in cmd/server/BUILD.buri — `buri gen //cmd/server` does this automatically
```

## Generated build files

`buri gen //lib/money` rewrites the fields that restate the sources and touches
nothing else. `buri docs cli gen` lists those fields.

**`gen` preserves the contents of `tags`, `platforms`, and `timeout_seconds`**,
along with `visibility`, `outputs`, and every comment. Somebody decided those
fields; you cannot derive them from the sources. So `buri gen //...` across the
whole repository can add and remove dependency edges, and cannot change what the
code is *allowed* to be. It does not preserve their *formatting*: `gen` leaves
the file as `buri format` would, so a `tags` list may come back rewrapped.

`gen` never invents a rule block, so a `BUILD.buri` has to exist before it will
write anything. An empty rule is enough to start:

```textproto schema=build
library {}
```
