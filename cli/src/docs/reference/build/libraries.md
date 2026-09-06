# Libraries: `lib.buri` and the public surface

A library is a package with a `lib.buri`. That file is the library's entire
public surface: everything a dependent can import, and nothing else.

## Two levels of export

[`language/modules.md` §4.2](../../language/modules.md) gives a declaration one
level of visibility: `export` shows it to modules that import the file. The
build system adds a second level above that:

| Level | Written | Visible to |
|---|---|---|
| Module | `export fn toCents(...)` in `cents.buri` | Any file **inside this library** that imports `//lib/money/cents.buri` |
| Library | `from "//lib/money/cents.buri" export { Cents };` in `lib.buri` | Any target that declares this library in `dependencies` |

Nothing changes about `export` itself. A library-level export is a re-export
([`language/modules.md` §4.2.1](../../language/modules.md)) written in a file
the build system knows the name of. The rule is mechanical: **if it is not
named in `lib.buri`, it is not reachable from outside the library.**

```buri repo=cli/tests/example package=//lib/money
// lib/money/lib.buri — the surface of //lib/money, complete.

from "//lib/money/cents.buri" export {
    add, Cents, format, fromCents, fromDollars, isZero,
};

from "//lib/money/parse.buri" export { parse, ParseError };
```

```buri
// lib/money/cents.buri

export struct Cents(I64); // name exported, contents not

export fn fromDollars(d: I64): Cents {
    Cents(d * 100)
}

export fn fromCents(c: I64): Cents {
    Cents(c)
}

// A method is declared inside an `impl` for its type, and that `impl` lives in
// the module that declares the type — which is what decides how a library's
// files are laid out.
impl Cents {
    export fn add(self, other: Cents): Cents {
        Cents(self.0 + other.0)
    }

    /// Exported so `parse.buri` can reach it, since the field is module-private.
    /// Absent from lib.buri, so it stops at the library boundary.
    export fn toCents(self): I64 {
        self.0
    }
}
```

From `//cmd/server`:

```buri repo=cli/tests/example package=//cmd/server
from "//lib/money" import { Cents, format }; // fine
from "//lib/money" import { toCents }; // ERROR: "//lib/money" does not export `toCents`
from "//lib/money/cents.buri" import { toCents }; // ERROR: internal to //lib/money
```

This buys one property: reviewing a library's API means reading one file. Not
grepping for `export` across forty. Not consulting a generated manifest that may
be stale. You open `lib.buri`, the same file the compiler reads.

## Module paths

There are no relative imports ([`language/modules.md`
§4.1.1](../../language/modules.md)). Every module path is absolute. A path means
the same module wherever you write it, and a file can move between directories
without changing its own imports.

You name a **surface** as a module. Everything else is a **file**, and only its
own package may name it. Both columns below are that one rule:

| Written | Resolves to | Legal from |
|---|---|---|
| `"core/list"` | A standard library module | Anywhere |
| `"//lib/money"` | The library's surface, `lib.buri` | Anywhere that declares the dependency and has visibility, the library's own suite included |
| `"//lib/money/testing"` | The library's test utilities | Only from a test source, anywhere |
| `"core/testing/assert"` | The test platform | Only from a test source, anywhere |
| `"//lib/money/cents.buri"` | One module inside it | Only from inside `//lib/money` |
| `"//cmd/server/main.buri"` | A binary's entry point | Only from that binary's own test sources |

So `//lib/money` does three jobs. It is a package path in a `BUILD.buri`. It is
an entry in `dependencies`, a *label* naming a target. It is the module path an
import writes for that library's surface. A library has one spelling in a
repository, and a suite reaches the library it tests by the same name its
dependents use.

Each way of writing a path wrong has its own answer. A path with a file
name left off is `import-path-without-a-file`, and the diagnostic names the file
it meant. No textual rule could: `"//lib/money/testing"` and
`"//lib/money/cents"` are the same shape, and only the layout says that one is a
surface and the other a file. A path that leaves the package and reaches a file
inside it is `internal-import`. The surface written the long way round,
`"//lib/money/lib.buri"`, is accepted and resolves to the same module. It is
unidiomatic rather than wrong. Nothing in the toolchain writes it, and nothing
in this repository contains one.

The rules the compiler enforces:

- **A `//pkg/...` import requires a matching `dependencies` entry** for `//pkg`
  in the importing target's rule, and visibility from the importing package.
  Both directions are errors: a use with no entry, and an entry nothing uses.
- **A `//pkg/inner.buri` import resolves only inside `//pkg`.** From outside,
  the diagnostic points at the library:

  ```
  error: //lib/money/cents.buri is internal to //lib/money [internal-import]
    --> lib/ledger/entry.buri:4:6
     |
   4 | from "//lib/money/cents.buri" import { Cents };
     |      ^^^^^^^^^^^^^^^^^^^^^^^^
     |
     = only names re-exported by lib/money/lib.buri are available
     = fix: import the library instead: from "//lib/money" import { ... }
  ```
- **A file and a package of the same name are two different modules.** If
  `lib/money/cents/` is a package, its surface is `//lib/money/cents` and the
  file beside it is `//lib/money/cents.buri`. These used to share one spelling,
  and you had to rename one of the two. They are two strings now, a surface
  named as a module and a file named as a file, and the layout is legal.
- **Rules inside a package do not reach into each other.** A binary imports the
  co-located library as `//pkg`, its surface, like any other dependent. Never
  `//pkg/render.buri`. The library may not import `//pkg/main.buri` at all.
- **A path containing a `testing` segment is importable only from a test
  source.** See below.
- **Circular imports are an error** at the module level already. Package cycles
  are the same rule one level up.

## The `testing/` surface

A library often has code that exists only to make *other people's* tests
possible: a fake, a fixture, a builder, a matcher. It belongs with the library,
because it changes when the library changes. It must also never reach a
production binary. One rule gives you both:

> **A module path containing a `testing` segment may be imported only from a
> test source.**

That covers every case, with no new field and nothing to remember to declare:

| Path | What it is |
|---|---|
| `//lib/ledger/testing` | A library's own test utilities |
| `//lib/testing/fakes` | A standalone package of shared test infrastructure |
| `core/testing/assert` | The test platform |

The restriction sits in the import line, where the person writing the import is
already looking, rather than in a build file three directories away. A
production module that reaches for one gets:

```
error: lib/store/file_store.buri imports a test-only module
  --> lib/store/file_store.buri:6:6
   |
 6 | from "//lib/ledger/testing" import { sample };
   |      ^^^^^^^^^^^^^^^^^^^^^^^
   |
   = a path containing `testing` may be imported only from a test source
   = lib/store/file_store.buri is in //lib/store's library sources
```

### A library's own `testing/`

```
lib/ledger/
  BUILD.buri
  lib.buri              <- //lib/ledger, from outside
  entry.buri
  testing/
    lib.buri            <- //lib/ledger/testing, from outside
    fixtures.buri
  test/
    ledger.buri
```

```textproto schema=build
library {
    sources: ["entry.buri", "posting/rules.buri"]
    dependencies: ["//lib/money"]

    test {
        sources: ["test/ledger.buri"]
    }

    testing {
        sources: ["testing/fixtures.buri"]
    }
}
```

`testing/lib.buri` is a second entry point of the same package, and behaves
exactly like the first one level down. It is the complete surface of
`//lib/ledger/testing`, made of re-exports, and a name absent from it is
unreachable.

```buri repo=cli/tests/example package=//lib/ledger role=test
// lib/ledger/testing/lib.buri — the surface of //lib/ledger/testing.
from "//lib/ledger/testing/fixtures.buri" export { oneOff, sample };
```

Living *in the package* rather than beside it buys four properties:

- **It may import the library's internals.** It can reach
  `//lib/ledger/entry.buri`, so you can build a fake out of the real thing
  without a back door in the public surface. That is the reason to prefer it
  over a separate package.
- **It has its own `dependencies`.** A fake usually needs less than the real
  implementation, and occasionally needs something else entirely. Those entries
  do not become dependencies of the library.
- **No production artifact ever links it.** It compiles only into test binaries,
  so it is a leaf in every non-test build and costs nothing there.
- **It inherits the library's `visibility` and `tags`.** Testing against
  `//lib/ledger` and depending on it take the same permission.

A consumer's test reaches it the way it reaches any library: declared, and by
label:

```textproto schema=build
# tools/report/BUILD.buri
library {
    sources: ["render.buri"]
    dependencies: ["//lib/ledger", "//lib/money"]

    test {
        sources: ["test/render.buri"]
        dependencies: ["//lib/ledger/testing"]
    }
}
```

The convention costs you a reserved directory name. A package cannot have a
product subdirectory called `testing`, and a package path cannot contain that
segment. That is the whole price, and you see it the moment you try.

## The re-export declaration

`lib.buri` is made of re-exports:

```buri repo=cli/tests/example package=//lib/money
from "//lib/money/cents.buri" export { Cents, fromCents };
from "//lib/money/cents.buri" export { add as addMoney };   // renaming is allowed
from "//lib/money/cents.buri" export *;                     // ERROR: expected `{`, found `*`
```

There is no `export *`, for the same reason there is no bare `import *`: every
name that enters or leaves a module is written in that module's source. Here it
also stops the surface growing by accident. Adding an `export` to an internal
module publishes nothing until someone edits `lib.buri`, and that edit shows up
in review as a change to the API, which is what it is.

A `lib.buri` may also declare things itself. It is an ordinary module that
happens to be the entry point, so this is fine:

```buri repo=cli/tests/example package=//lib/money
from "//lib/money/cents.buri" export { Cents, fromCents };

from "//lib/money/cents.buri" import { Cents, toCents };

/// Declared here rather than re-exported; both are public surface. A free
/// function, not a method: `Cents` is declared in cents.buri, and a method must
/// live in its type's defining module ([§6.7.3](../../language/expressions.md)).
export fn isRound(c: Cents): Bool {
    c.toCents() % 100 == 0
}
```

Three consequences worth spelling out:

- **The surface filters methods, like everything else.** Method calls resolve
  through the receiver's defining module rather than through scope
  ([`language/expressions.md` §6.7](../../language/expressions.md)). Without
  this rule a type could smuggle operations across the boundary: `from
  "//lib/money" import { Cents }` would make every method on `Cents` callable,
  `toCents` included, whether or not `lib.buri` mentioned it. So the rule is
  uniform: **a name is on the surface if `lib.buri` exports it, and a method
  call from outside the library resolves only to names on the surface.**
  Exporting `add` gives you both `add(a, b)` and `a.add(b)`. Leaving out
  `toCents` removes both.

  Resolution itself does not change: still one type, one module, one lookup,
  with a visibility filter applied after it. That is exactly what `export`
  already does one level down, where another module cannot call a private method
  either. The cost is listing a library's methods in `lib.buri` one at a time.
  They are also its API, so you were going to list them anyway.

  Inside the library, [`language/modules.md` §4.1](../../language/modules.md)
  applies unchanged: importing `Cents` from `//lib/money/cents.buri` brings all
  of its exported methods, `toCents` included.

- **Member visibility and the library boundary are different mechanisms** that
  compose. `export` on a field hides a representation from every module, its own
  library's included. The library boundary hides a name from every target but
  its own. `Cents` is both: internal code can construct one and cannot see
  inside it.

- **A method on an unexported type is unreachable.** That is the intended
  behavior, and `dead-code`, a lint every repository runs, reports it.

One layout consequence surprises people once and then never again. A type's
methods must be declared in the module that declares the type, so `Cents` and
everything spelled `c.something()` live in one file, however long it gets.
Functions *over* a type go anywhere. That includes functions over `[Cents]`,
which can never be methods at all, since the defining module of `[T]` is
`core/list`. A library's file layout follows its types, not its verbs:

```
lib/money/
  cents.buri     the Cents type and every method on it
  parse.buri     free functions producing a Cents
  batch.buri     free functions over [Cents]
```

## Subdirectories

A library can be as many directories deep as it likes. Only a `BUILD.buri`
creates a package.

```
lib/ledger/
  BUILD.buri
  lib.buri
  entry.buri
  posting/
    rules.buri
    interest.buri
  test/
    ledger.buri
```

```textproto schema=build
library {
    sources: [
        "entry.buri",
        "posting/interest.buri",
        "posting/rules.buri",
    ]
    dependencies: ["//lib/money"]
    visibility: ["//cmd/...", "//lib/store"]

    test {
        sources: ["test/ledger.buri"]
    }
}
```

Within the library, `entry.buri` imports `//lib/ledger/posting/rules.buri`, and
`rules.buri` imports `//lib/ledger/entry.buri`. Absolute paths, no visibility
rules, no build graph. `lib.buri` re-exports from wherever the names live:

```buri repo=cli/tests/example package=//lib/ledger
// lib/ledger/lib.buri
from "//lib/ledger/entry.buri" export { Entry, entry, total };

from "//lib/ledger/posting/rules.buri" export { apply, Rule };
```

Subdirectory nesting costs nothing in the build graph: one library is one
compile action set, however you arrange its files. Split a directory into a
package when you want a **boundary**: a different visibility, a different tag, a
separate test suite, a cache edge that stops churn from propagating. Not when a
directory merely holds a lot of files.

## The `test/` directory

`test/` is reserved. A `.buri` file under `test/` must appear in a rule's
`test.sources` and may not appear in `sources`. Nothing may import it, not even
another test source, since the compiler compiles each one independently. See
[`testing.md`](./testing.md).
