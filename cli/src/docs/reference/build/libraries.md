# Libraries: `lib.buri` and the public surface

A library is a package with a `lib.buri`. That file is the library's entire
public surface.

## Two levels of export

`export` ([`language/modules.md` §4.2](../../language/modules.md)) shows a
declaration to modules that import its file. The build system adds a second
level:

| Level | Written | Visible to |
|---|---|---|
| Module | `export fn toCents(...)` in `cents.buri` | Any file **inside this library** that imports `//lib/money/cents.buri` |
| Library | `from "//lib/money/cents.buri" export { Cents };` in `lib.buri` | Any target that declares this library in `dependencies` |

A library-level export is a re-export
([`language/modules.md` §4.2.1](../../language/modules.md)) in `lib.buri`.
**If it isn't named in `lib.buri`, it isn't reachable from outside the
library.**

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

// Methods live in an `impl` in the module that declares the type.
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

Reviewing a library's API means reading one file.

## Module paths

Every module path is absolute ([`language/modules.md`
§4.1.1](../../language/modules.md)), so a path means the same module wherever
you write it. You name a **surface** as a module. Everything else is a
**file**, and only its own package may name it:

| Written | Resolves to | Legal from |
|---|---|---|
| `"core/list"` | A standard library module | Anywhere |
| `"//lib/money"` | The library's surface, `lib.buri` | Anywhere that declares the dependency and has visibility, the library's own suite included |
| `"//lib/money/testing"` | The library's test utilities | Only from a test source, anywhere |
| `"core/testing/assert"` | The test platform | Only from a test source, anywhere |
| `"//lib/money/cents.buri"` | One module inside it | Only from inside `//lib/money` |
| `"//cmd/server/main.buri"` | A binary's entry point | Only from that binary's own test sources |

`//lib/money` is the package path in a `BUILD.buri`, the entry in
`dependencies`, and the module path an import writes. A suite reaches the
library it tests by the same name its dependents use.

A path with the file name left off is `import-path-without-a-file`, and the
diagnostic names the file it meant. Reaching into another package's file is
`internal-import`. `"//lib/money/lib.buri"` also resolves to the surface; it's
unidiomatic, not wrong.

The compiler enforces:

- **A `//pkg/...` import requires a matching `dependencies` entry** for `//pkg`
  in the importing target's rule, and visibility from the importing package.
  A use with no entry is an error, and so is an entry nothing uses.
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
  file beside it is `//lib/money/cents.buri`.
- **Rules inside a package don't reach into each other.** A binary imports the
  co-located library as `//pkg`, like any dependent, never `//pkg/render.buri`.
  The library may not import `//pkg/main.buri` at all.
- **A path containing a `testing` segment is importable only from a test
  source.** See below.
- **Circular imports are an error**, between modules and between packages.

## The `testing/` surface

Fakes, fixtures, builders and matchers for *other people's* tests belong with
the library, since they change with it, but must never reach a production
binary. One rule gives you both:

> **A module path containing a `testing` segment may be imported only from a
> test source.**

| Path | What it is |
|---|---|
| `//lib/ledger/testing` | A library's own test utilities |
| `//lib/testing/fakes` | A standalone package of shared test infrastructure |
| `core/testing/assert` | The test platform |

A production module that imports one gets:

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

`testing/lib.buri` is a second entry point of the same package and works
exactly like the first: it's the complete surface of `//lib/ledger/testing`,
made of re-exports.

```buri repo=cli/tests/example package=//lib/ledger role=test
// lib/ledger/testing/lib.buri — the surface of //lib/ledger/testing.
from "//lib/ledger/testing/fixtures.buri" export { oneOff, sample };
```

Because it lives *in the package*:

- **It may import the library's internals**, like `//lib/ledger/entry.buri`, so
  a fake can wrap the real thing without a back door in the public surface.
- **It has its own `dependencies`**, which don't become the library's.
- **No production artifact links it.** It compiles only into test binaries.
- **It inherits the library's `visibility` and `tags`**, so testing against
  `//lib/ledger` takes the same permission as depending on it.

A consumer's test declares it like any library:

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

The cost is a reserved name: a package can't have a product subdirectory
called `testing`, and a package path can't contain that segment.

## The re-export declaration

`lib.buri` is made of re-exports:

```buri repo=cli/tests/example package=//lib/money
from "//lib/money/cents.buri" export { Cents, fromCents };
from "//lib/money/cents.buri" export { add as addMoney };   // renaming is allowed
from "//lib/money/cents.buri" export *;                     // ERROR: expected `{`, found `*`
```

There's no `export *`, just as there's no bare `import *`: every name entering
or leaving a module is written in its source. Adding an `export` to an internal
module publishes nothing until someone edits `lib.buri`, and review sees that
edit as an API change.

`lib.buri` is an ordinary module, so it may declare things itself:

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

- **The surface filters methods too.** Method calls resolve through the
  receiver's defining module, not scope
  ([`language/expressions.md` §6.7](../../language/expressions.md)), so without
  this a type could smuggle operations across the boundary. **A method call
  from outside the library resolves only to names `lib.buri` exports.**
  Exporting `add` gives you both `add(a, b)` and `a.add(b)`; leaving out
  `toCents` removes both.

  Inside the library, [`language/modules.md` §4.1](../../language/modules.md)
  applies unchanged: importing `Cents` from `//lib/money/cents.buri` brings all
  of its exported methods, `toCents` included.

- **Member visibility and the library boundary compose.** An unexported field
  hides a representation from every other module, even in its own library. The
  library boundary hides a name from every other target. `Cents` gets both:
  internal code can construct one but can't see inside it.

- **A method on an unexported type is unreachable**, and the `dead-code` lint
  reports it.

`Cents` and every `c.something()` method live in one file, however long it
gets. Functions *over* a type go anywhere, including functions over `[Cents]`,
which can't be methods because `[T]` is defined in `core/list`. A library's file
layout follows its types, not its verbs:

```
lib/money/
  cents.buri     the Cents type and every method on it
  parse.buri     free functions producing a Cents
  batch.buri     free functions over [Cents]
```

## Subdirectories

A library can nest directories freely. Only a `BUILD.buri` creates a package.

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

Inside the library, files import each other by absolute path, such as
`//lib/ledger/posting/rules.buri`, with no visibility rules. `lib.buri`
re-exports from wherever the names live:

```buri repo=cli/tests/example package=//lib/ledger
// lib/ledger/lib.buri
from "//lib/ledger/entry.buri" export { Entry, entry, total };

from "//lib/ledger/posting/rules.buri" export { apply, Rule };
```

Nesting costs nothing in the build graph. Split a directory into its own
package when you want a **boundary**: a different visibility or tag, a separate
test suite, or a cache edge that stops churn from propagating.

## The `test/` directory

`test/` is reserved. A `.buri` file under it must appear in a rule's
`test.sources`, not `sources`. Nothing may import it, not even another test
source, since each one compiles independently. See
[`testing.md`](./testing.md).
