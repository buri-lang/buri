# Using the build system

A Buri repository declares which files a target compiles, which libraries it
may use, and who may use it. Nothing is discovered, and there are three rule
kinds instead of a rule language. The full rules are in
[`reference/build/overview.md`](../reference/build/overview.md) and
[`reference/build/build-files.md`](../reference/build/build-files.md).

## The root, and what a package is

`REPO.buri`, written by `buri init`, marks the root that `//` resolves against.
It holds the tag vocabulary and lint policy
([`repo-config.md`](../reference/build/repo-config.md)).

**A directory holding a `BUILD.buri` is a package**, the unit you build, test
and depend on:

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

A subdirectory without its own `BUILD.buri` is no boundary:
`posting/rules.buri` is a source of `//lib/ledger`, listed by that path.

`lib.buri` is a library's public surface and `main.buri` a binary's entry
point. A package holds at most one of each, so a label never needs a target
name.

## A library, end to end

`lib/money/BUILD.buri` is all the build system knows about it:

```textproto schema=build
library {
    sources: ["cents.buri", "parse.buri"]
    visibility: ["//visibility:public"]

    test {
        sources: ["test/cents.buri", "test/parse.buri"]
    }
}
```

`lib.buri` is implied, so it isn't in `sources`. Every other file is listed, and
a `.buri` file no rule lists is an error that `buri gen` fixes.

The surface is a file, not a field:

```buri repo=cli/tests/example package=//lib/money
//! The public surface of //lib/money. cents.buri also exports `toCents`, but
//! only this library can see it.

from "//lib/money/cents.buri" export {
    add, Cents, format, fromCents, fromDollars, isZero,
};

from "//lib/money/parse.buri" export { parse, ParseError };
```

Module paths are absolute. Other packages import the surface as
`//lib/money`; a path like `//lib/money/cents.buri` resolves only inside the
library. An internal file exports what the library needs, and `lib.buri`
decides what leaves the package:

```buri
/// The field isn't exported, so no caller can add a Cents to an I64.
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

    /// `parse.buri` can call this. lib.buri doesn't re-export it, so other
    /// packages can't, as a function or as a method.
    export fn toCents(self): I64 {
        self.0
    }
}
```

[`libraries.md`](../reference/build/libraries.md) has the re-export forms and
how imports resolve.

## A binary

Nothing may depend on a binary, so it declares its artifacts instead of a
visibility list:

```textproto schema=build
binary {
    sources: ["routes.buri"]
    dependencies: ["//lib/ledger", "//lib/money", "//lib/store"]
    tags: ["server"]

    outputs: [
        { platform: "native", variant: "linux-x86_64" },
        { platform: "native", variant: "macos-arm64" },
    ]

    test {
        sources: ["test/routes.buri"]
    }
}
```

`main.buri` is required, exports `main`, and isn't in `sources`. Each output is
checked separately, so `buri build //cmd/server` produces two artifacts and can
fail for one only. `tags` say what the code *is*, and `REPO.buri` declares what
follows ([`tags-policy.md`](./tags-policy.md),
[`tags.md`](../reference/build/tags.md)).

A package may hold both rules; the binary still imports the library by its
label.

## Labels and patterns

A label is a package path and never carries a target name:

| Written | Means |
|---|---|
| `//lib/money` | In `dependencies`, that package's library. On the command line, every target in it. |
| `//lib/...` | Every target under `lib/`, including `lib` itself |
| `//...` | Every target in the repository |

Patterns work on the command line, never in a build file:

```sh
buri build //...                  every target
buri test //lib/money             one package's suites
buri run //cmd/server             build and run the binary
buri query 'rdeps(//lib/money)'   what would break if this changed
```

They work from any directory.

## Adding a dependency

The compiler checks three things.

**You use it.** Importing isn't the only use. A method resolves through its
receiver's type, so calling `format` on a `Cents` uses `//lib/money` even when
no import names it:

```buri repo=cli/tests/example
# from "platform/effect" import { Allocator };
from "//lib/ledger" import { Entry };

// `amount` is a Cents from //lib/money, and `format` is one of its methods —
// no import names //lib/money, and this target still depends on it.
fn line<C: Allocator>(ctx: C, e: Entry): Str {
    e.amount.format(ctx)
}
```

**You declare it.** Add the label to `dependencies`, or let `buri gen` do it.
An entry no source uses is an error too. `core/*` ships with the toolchain and
is never listed.

```
error: cmd/server/routes.buri imports //lib/money, which is not in dependencies
  --> cmd/server/routes.buri:3:6
   |
 3 | from "//lib/money" import { Cents, format };
   |      ^^^^^^^^^^^^^
   |
   = fix: add "//lib/money" to dependencies in cmd/server/BUILD.buri — `buri gen //cmd/server` does this automatically
```

**You're allowed to.** The dependency's `visibility` decides who may depend on
it. Without one, a rule is private to its package:

```textproto schema=build
# lib/store/BUILD.buri — the database layer is not for general use
library {
    sources: ["codec.buri", "file_store.buri"]
    dependencies: ["//lib/ledger", "//lib/money"]
    tags: ["server"]
    visibility: ["//cmd/server"]
}
```

The error names the file to change, which is the library's:

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

Widen it with a pattern (`//cmd/...`) or the one package, or move the shared
code to a library both may see.

## Let `buri gen` write the boring fields

`buri gen` rewrites `sources`, `dependencies` and their `test` and `testing`
counterparts from the files on disk and the imports they write:

```sh
buri gen              # the whole repository, the same as `buri gen //...`
buri gen //lib/money  # one package
buri gen --check      # writes nothing; exits 1 if anything would change
```

Run it after adding a file or an import, and in CI as `--check`.

- **It never invents a rule block.** A new package needs a `BUILD.buri` first;
  an empty `library {}` is enough.
- **It never touches a decision.** `generators`, `tags`, `platforms`,
  `visibility`, `outputs`, `timeout_seconds` and comments survive, so
  `buri gen //...` never widens what a library is allowed to be.

Most build-file work looks like this: write the code, run `buri gen`, read the
diff.

## Next

- [Testing your code](./testing.md): the `test` block, and what a suite may
  reach.
- [Tags and policy](./tags-policy.md): keeping server code out of the browser
  bundle.
- [Reproducible builds](./reproducibility.md): what the cache keys on.
- The exact rules: [the build model](../reference/build/overview.md),
  [`BUILD.buri`](../reference/build/build-files.md),
  [libraries](../reference/build/libraries.md), and
  [`REPO.buri`](../reference/build/repo-config.md).
