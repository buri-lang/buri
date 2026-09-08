# Using the build system

You declare everything in a Buri repository: which files a target compiles,
which libraries it may use, and who may use it. Nothing is discovered by walking
the filesystem, and there are two rule kinds rather than a rule language.

The exact rules are in
[`reference/build/overview.md`](../reference/build/overview.md) and
[`reference/build/build-files.md`](../reference/build/build-files.md): every
schema field and every visibility pattern.

## The root, and what a package is

`REPO.buri` makes a directory the repository root. `//` in every label and every
module path resolves against it, so a name means the same thing typed from any
subdirectory. `buri init` writes one, and it holds the tag vocabulary and the
lint policy and nothing else
([`repo-config.md`](../reference/build/repo-config.md)).

Below it, **a directory holding a `BUILD.buri` is a package**. That is the unit
you build, test, and depend on:

```
REPO.buri
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
      routes.buri
```

A subdirectory with no `BUILD.buri` of its own is not a boundary of anything:
`posting/rules.buri` is a source of `//lib/ledger`, listed by that path. Split a
growing library into directories freely.

Two filenames are fixed. `lib.buri` is a library's public surface, and
`main.buri` is a binary's entry point. A package holds at most one of each,
which is why a label never needs a target name.

## A library, end to end

`lib/money/BUILD.buri` is the whole of what the build system knows about it:

```textproto schema=build
library {
    sources: ["cents.buri", "parse.buri"]
    visibility: ["//visibility:public"]

    test {
        sources: ["test/cents.buri", "test/parse.buri"]
    }
}
```

`lib.buri` is absent from `sources` because the `library` rule kind names it.
You list everything else one path at a time, and a `.buri` file no rule lists is
an error rather than a file quietly left out of the build.

The surface is a file, not a field:

```buri repo=cli/tests/example package=//lib/money
//! The public surface of //lib/money. A dependent can import these names and no
//! others; `toCents` below is exported by cents.buri but not from here, so it is
//! visible inside this library and nowhere else.

from "//lib/money/cents.buri" export {
    add, Cents, format, fromCents, fromDollars, isZero,
};

from "//lib/money/parse.buri" export { parse, ParseError };
```

Module paths are absolute; there are no relative imports. A library's surface is
a module, `//lib/money`. Every other file is named by its path inside the
package, `//lib/money/cents.buri`, and that path resolves only from within the
library.

So an internal file exports whatever the rest of the library needs, and
`lib.buri` decides which of that leaves the package:

```buri
/// Money is never a raw integer. The field is not exported, so no caller can
/// add a Cents to an I64 by accident.
export struct Cents(I64);

export fn fromDollars(d: I64): Cents {
    Cents(d * 100)
}

export fn fromCents(c: I64): Cents {
    Cents(c)
}

impl Cents {
    export fn add(self, other: Cents): Cents {
        Cents(self.0 + other.0)
    }

    /// Exported from this module, so `parse.buri` can reach it. Not re-exported
    /// from lib.buri, so it is invisible outside //lib/money — as a free function
    /// and as a method.
    export fn toCents(self): I64 {
        self.0
    }
}
```

A name `lib.buri` withholds does not resolve in another package, as a free
function or as a method. [`libraries.md`](../reference/build/libraries.md) has
the re-export forms and how imports resolve.

## A binary

A binary declares the artifacts it produces instead of a visibility list,
because nothing may depend on a binary:

```textproto schema=build
binary {
    sources: ["routes.buri"]
    dependencies: ["//lib/ledger", "//lib/money", "//lib/store"]
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

`main.buri` is required, exports `main`, and is not listed in `sources`. The
compiler checks each entry in `outputs` separately against the whole dependency
graph, so `buri build //cmd/server` here produces two artifacts and can succeed
for one platform while failing for another. `tags` say what the code *is*, and
what follows from a tag is declared once in `REPO.buri`
([`tags-policy.md`](./tags-policy.md) is the task,
[`tags.md`](../reference/build/tags.md) the rules).

A package may hold both rules, and even then the binary reaches its neighbour
only through the label anybody else writes.

## Labels and patterns, in daily use

A label is a package path and never carries a target name:

| Written | Means |
|---|---|
| `//lib/money` | In `dependencies`, that package's library. On the command line, every target in it. |
| `//lib/...` | Every target under `lib/`, including `lib` itself |
| `//...` | Every target in the repository |

You may write a pattern on the command line, never in a build file:

```sh
buri build //...                  every target
buri test //lib/money             one package's suites
buri run //cmd/server             build and run the binary
buri query 'rdeps(//lib/money)'   what would break if this changed
```

Labels are absolute, so those commands mean the same thing from any directory,
and an import writes the same string: `from "//lib/money" import { Cents };`.

## Adding a dependency

Three things have to agree, and the compiler checks all three.

**Use it.** An import is the usual way, but not the only one: a method resolves
through its receiver's type, so calling `e.amount.format(ctx)` on a `Cents` that
arrived from `//lib/money` uses that library even when no import names it:

```buri repo=cli/tests/example
# from "core/effect" import { Allocator };
from "//lib/ledger" import { Entry };

// `amount` is a Cents from //lib/money, and `format` is one of its methods —
// no import names //lib/money, and this target still depends on it.
fn line<C: Allocator>(ctx: C, e: Entry): Str {
    e.amount.format(ctx)
}
```

**Declare it.** Add the label to `dependencies`, or let `buri gen` do it. An
entry no source uses is an error too.

```
error: cmd/server/routes.buri imports //lib/money, which is not in dependencies
  --> cmd/server/routes.buri:3:6
   |
 3 | from "//lib/money" import { Cents, format };
   |      ^^^^^^^^^^^^^
   |
   = fix: add "//lib/money" to dependencies in cmd/server/BUILD.buri — `buri gen //cmd/server` does this automatically
```

`core/*` is the exception: it ships with the toolchain, is available everywhere,
and is never listed.

**Be allowed to.** The `visibility` on the library you depend on decides who may
write that edge, and a rule that omits it is private to its own package:

```textproto schema=build
# lib/store/BUILD.buri — the database layer is not for general use
library {
    sources: ["codec.buri", "file_store.buri"]
    dependencies: ["//lib/ledger", "//lib/money"]
    tags: ["server"]
    visibility: ["//cmd/server"]
}
```

The diagnostic names the file that has to change, which is the library's:

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

Widen it with a pattern (`//cmd/...`), or name the one package that needs it.
Often the better answer is a library both may see.

## Let `buri gen` write the boring fields

`buri gen` rewrites the fields that merely restate the source tree — `sources`,
`dependencies`, and their `test` and `testing` counterparts — from the files
that exist and the imports they write:

```sh
buri gen              # the whole repository, the same as `buri gen //...`
buri gen //lib/money  # one package
buri gen --check      # writes nothing; exits 1 if anything would change
```

Run it after adding a file, after adding an import, and in CI as `--check`. Two
habits make it dependable:

- **It never invents a rule block**, so a new package needs a `BUILD.buri`
  before `gen` will write to it. An empty `library {}` is enough to start.
- **It never touches a decision.** `generators`, `tags`, `platforms`,
  `visibility`, `outputs`, `timeout_seconds`, and every comment survive, so
  `buri gen //...`
  can rewrite dependency edges across the repository without widening what any
  library is allowed to be.

A new file is undeclared until something lists it, and the error says so:

```
error: lib/ledger/posting/interest.buri is not declared by any rule
  --> lib/ledger/BUILD.buri
   |
   = add it to the library's sources, or delete it
   = run `buri gen //lib/ledger` to do this automatically
```

Most build-file work is that shape: write the code, run `buri gen`, read the
diff.

## Next

- [Testing your code](./testing.md) — the `test` block, and what a suite may
  reach.
- [Tags and policy](./tags-policy.md) — keeping server code out of the browser
  bundle.
- [Reproducible builds](./reproducibility.md) — what the cache keys on.
- The exact rules: [the build model](../reference/build/overview.md),
  [`BUILD.buri`](../reference/build/build-files.md),
  [libraries](../reference/build/libraries.md), and
  [`REPO.buri`](../reference/build/repo-config.md).
