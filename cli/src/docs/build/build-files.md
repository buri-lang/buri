# `BUILD.buri`

The schema is [`schema/build.proto`](../schema/build.proto); this document is the
prose version, with the reasoning.

## The file

`BUILD.buri` is textproto parsing as `buri.build.v1.BuildFile`. It has no
expression language: no variables, no conditionals, no string concatenation, no
globs, no `load`, no rule authoring. Everything a rule depends on is written out
in the rule. The cost is repetition, and it is paid by `buri gen` writing most
of it. `sources: ["*.buri"]` in particular is not accepted, because a glob makes
the file list depend on the state of the filesystem — which is precisely the
input hermeticity is trying to pin down.

Textproto over a bespoke syntax because the schema is then a real artifact
rather than documentation — the parser rejects an unknown field with a line
number, an editor completes field names, and the CLI reads build files with the
same `.proto` machinery the language uses for wire formats.

```textproto schema=build
# lib/money/BUILD.buri

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

`#` starts a comment. `buri format` formats build files as well as source: the
schema's field order, one field per line, trailing commas, four-space indent,
and every list left in the order it was written. `buri docs cli format` has the
whole of it, and `buri docs cli gen` covers the sorting, which belongs to `gen`
because it is `gen` that decides what a managed list contains.

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

Subdirectories are for organizing a library that has grown, and cost nothing:
no rule, no visibility, no dependency edge. The only thing that creates a
boundary is a `BUILD.buri`.

A package declares **at most one library and at most one binary**. That falls
out of the fixed entry-point filenames — one `lib.buri` per directory, one
`main.buri` per directory — and it is what makes a label a bare path.

## Labels

A label is a package path. It never carries a target name:

```
//lib/money        the library in package lib/money
//cmd/server       the package cmd/server
//lib              the package lib, if one has a BUILD.buri there
```

In a `dependencies` list a label always means **the library** of that package,
because a library is the only thing that can be depended on. In a CLI argument it means
**every target** in that package. Those are the only two contexts a label
appears in, and neither is ambiguous, so there is no `:name` syntax to learn and
no rule about when to omit it.

For the same reason **a rule has no `name` field**. A package holds at most one
library and at most one binary, so the package path and the rule kind already
identify a target: `//lib/money` is the library, `//cmd/server` is the binary,
and that is what diagnostics print. A `name` would be a second identifier for a
target that already has one, free to drift from the directory it sits in and
useful for addressing nothing. Where a filename is genuinely needed — the
artifact a binary produces — it defaults to the package's directory name and is
overridden on the output that wants it, not on the rule:

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

Labels are always repository-absolute. There is no relative label form: a label
means the same thing wherever it is written, including in a CLI invocation from
a subdirectory.

Module paths in source use the same spelling — `from "//lib/money/lib.buri" import …` —
and resolve to that library's `lib.buri`. See
[`libraries.md`](./libraries.md#module-paths).

## `library`

```textproto schema=build
library {
  sources: [
    "entry.buri",
    "posting/rules.buri",
  ]
  dependencies: ["//lib/money"]
  tags: ["server"]
  visibility: ["//cmd/...", "//lib/reporting"]

  test {
    sources: ["test/ledger.buri"]
    dependencies: ["//lib/testing/fakes"]
    data: ["test/golden/ledger.txt"]
  }
}
```

| Field | Meaning |
|---|---|
| `sources` | Every `.buri` file in the package that belongs to this library, **excluding** `lib.buri` and the test sources. Package-relative, may descend into subdirectories. |
| `proto_sources` | Every `.proto` schema in the package that belongs to this library. Each becomes a module named by its own path, holding the types it declares and their codecs. See [`proto.md`](./proto.md). |
| `dependencies` | Labels of libraries this one may use. |
| `tags` | Labels saying what this code is; the policy they carry is declared in `REPO.buri`. See [`tags.md`](./tags.md). |
| `platforms` | The platforms it can be built for. Omit unless the code is genuinely platform-specific — unset means all of them. |
| `visibility` | Who may depend on it. Defaults below. |
| `test` | The test suite for this library. See [`testing.md`](./testing.md). |
| `testing` | The library's utilities *for other people's tests*, rooted at `testing/lib.buri`. See below. |

`lib.buri` is required and is not listed in `sources`. The rule kind names the
entry point the way `binary` names `main.buri`: it is not an input among
others, it is the thing the rule is *about*. Listing it would also make it
possible to write a `library` without one, which is not a state the build system
wants to have a diagnostic for.

Every other `.buri` file in the package must appear in exactly one rule's
`sources`, `test.sources`, or `testing.sources`, and every `.proto` in exactly
one rule's `proto_sources`. A file that appears in none is an error —

```
error: lib/ledger/posting/interest.buri is not declared by any rule
  --> lib/ledger/BUILD.buri
   |
   = add it to the library's sources, or delete it
   = run `buri gen //lib/ledger` to do this automatically
```

— and a file that appears in two is an error as well. The alternative, ignoring
undeclared files, means a file can be dropped from the build by a typo in a path
and never noticed.

### The `testing` block

A library's utilities for *other people's* tests live in a `testing/`
subdirectory with its own entry point, declared by a `testing` block:

```textproto schema=build
library {
  sources: ["entry.buri", "posting/rules.buri"]
  dependencies: ["//lib/money"]
  visibility: ["//cmd/...", "//lib/store", "//tools/report"]

  testing {
    sources: ["testing/fixtures.buri"]
  }

  test {
    sources: ["test/ledger.buri"]
  }
}
```

The schema-level rules: `testing/lib.buri` is required when the block is
present and is not listed in `sources`; the block is required when the file
exists; and it may be empty (`testing {}`) if the entry point is the whole of
it. The surface it declares, what those modules may import, and why the
restriction is carried by the path rather than by a `testonly` field are in
[`libraries.md`](./libraries.md#the-testing-surface).

## `binary`

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
    { platform: LINUX, arch: ARM64 },
    { platform: MACOS, arch: ARM64 },
  ]

  test {
    sources: ["test/routes.buri"]
  }
}
```

`main.buri` is required, is not listed in `sources`, and must export `main` with
the signature [`SPEC.md` §11](../SPEC.md) requires: no parameters, returning
`Result<(), Str>`. It is also the only module in the binary that may import
`core/host`, and the context it builds there is checked against the platform for
**each output**.

A platform *is* the set of effects its host exports: a platform that does not
grant one does not export the name for it, so asking for it is an ordinary
unresolved name at the line that asked, reported as `host-not-granted`. A `main`
binding `Ui: host.ui` under `platform: JS` does not compile, and neither does one
binding `Net: host.net` under `platform: WEB`. `buri docs error host-not-granted`
has the table of what each platform grants, and the reasoning.

`outputs` is a list because one entry point commonly ships several ways. Each
entry names a platform, and the whole dependency graph is checked against it
separately, so `buri build //cmd/server` may succeed for Linux and fail for JS.
Build one with `buri build //cmd/server --output=js`. A binary has no
`platforms` field of its own: `outputs` already says.

`tags` on a binary mean exactly what they mean on a library — labels saying what
the code is — and are covered in [`tags.md`](./tags.md). There is no second tag
mechanism for binaries. The tag check does not vary across outputs, so it runs
once no matter how many artifacts the binary produces.

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
  outputs: [{ platform: LINUX, arch: X86_64 }]

  test {
    sources: ["test/flags.buri"]
  }
}
```

Two rules, one directory, one build file, and still no names — the rule kind
tells the two apart, and the binary's artifact is `report` after the directory.
The rules are:

- **The `sources` sets are disjoint.** Every file belongs to exactly one rule.
- **The binary implicitly depends on the co-located library.** It does not
  appear in `dependencies`; a self-edge inside a package would be the only label
  in the system pointing at itself.
- **The binary reaches the library only through `//tools/report`.** `main.buri`
  may import that label — the library's entry point — and may not import
  `//tools/report/render`. The library boundary is a property of the library,
  not of the directory, so it holds even for a file sitting next to it.
- **The library may not reach the binary at all.** `lib.buri` importing
  `//tools/report/main` is an error.

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
default and no repository default — the one place that decides who may depend on
a library is the library's own rule. A default declared elsewhere would mean
`visibility` being absent tells you nothing until you have found and read
another file, which is the opposite of what putting it on the rule is for.

The cost is repetition in a package that opens several surfaces the same way,
paid because the alternative is a repository whose surfaces can be widened by
editing a file that names none of them.

```textproto schema=build
# lib/store/BUILD.buri — the database layer is not for general use

library {
  sources: ["codec.buri", "file_store.buri"]
  dependencies: ["//lib/ledger", "//lib/money"]
  tags: ["server"]
  visibility: ["//cmd/server"]
}
```

The diagnostic names the rule that has to change, since that is where the
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

Two edges skip the check, because neither is a dependency anyone chose: a
target's own test suite reaching the target under test, and a binary reaching
the library in its own package. Everything else, including a test suite reaching
a library named in `test.dependencies`, is checked normally.

Visibility is checked on the **declared edge**, not transitively. If `//cmd/web`
depends on `//lib/ledger` and `//lib/ledger` depends on `//lib/store`, then
`//lib/store` needs to be visible to `//lib/ledger` and to nobody else.
Restricting what may travel through a transitive chain is what tags are for, and
they are the right tool for it because the constraint follows the code rather
than the edge — a tag is checked over the whole closure, so it does not matter
who wrote the edge that pulled the code in.

## Dependencies

- `dependencies` lists **libraries only**, and a label there always resolves to
  one. A binary is not a valid dependency.
- **Use is what requires a dep, and an import is not the only way to use.** A
  method resolves through its receiver's type rather than through scope, so

  ```buri repo=cli/tests/example
# from "core/effect/lib.buri" import { Alloc };
  from "//lib/ledger/lib.buri" import { Entry };
  // `amount` is a Cents from //lib/money, and `format` is one of its methods —
  // no import names //lib/money, and this target still depends on it.
  fn line<C: Alloc>(ctx: C, e: Entry): Str { e.amount.format(ctx) }
  ```

  requires `//lib/money` in `dependencies` as much as an import would.
  Dependencies are direct: a library you use is one you declare, whether or not
  something else in the graph also happens to pull it in.
- `core/*` ships with the toolchain and is never listed. It is available to
  every target, and the purity tiers in [`SPEC.md` §11.1](../SPEC.md) already
  govern what any given import of it can do.
- **Cycles are an error**, at the package level exactly as at the module level.
  The diagnostic prints the cycle in the order the edges were declared.
- **Every entry must be used, and every use must have an entry.** Using
  `//lib/money` with nothing matching in `dependencies` is an error at the use
  site; a `dependencies` entry no source uses is an error at the build file.
  Both are errors and not warnings, because both make the dependency graph a
  description of something other than the code, and `buri gen` fixes either in
  one command.

```
error: cmd/server/routes.buri imports //lib/money, which is not in dependencies
  --> cmd/server/routes.buri:3:6
   |
 3 | from "//lib/money/lib.buri" import { Cents, format };
   |      ^^^^^^^^^^^^^
   |
   = fix: add "//lib/money" to dependencies in cmd/server/BUILD.buri — `buri gen //cmd/server` does this automatically
```

## Generated build files

`buri gen //lib/money` rewrites the fields that restate the sources and touches
nothing else. `buri docs cli gen` covers which fields those are and how a file
with both rules in it is divided; what matters *to a build file* is the other
half of the split.

**The contents of `tags`, `platforms`, and `timeout_seconds` are preserved**,
along with `visibility`, `outputs`, `test.data`, and every comment. Those fields
are decisions somebody made rather than facts derivable from the sources, and a
tool that dropped a `tags` entry while tidying `sources` would silently widen
what a library is allowed to link into. So `buri gen //...` across the whole
repository can add and remove dependency edges; it cannot change what the code
is *allowed* to be. Their *formatting* is not preserved and is not meant to be:
`gen` leaves the file as `buri format` would, so a `tags` list may come back
rewrapped. What survives is what the field says, not how it was typed.

The rule blocks themselves are never invented, so a `BUILD.buri` has to exist
before `gen` will write to it. An empty rule is enough to start:

```textproto schema=build
library {}
```
