# Libraries: `lib.buri` and the public surface

A library is a package with a `lib.buri`. That file is the library's entire
public surface — everything a dependent can import, and nothing else.

## Two levels of export

[`SPEC.md` §4.2](../SPEC.md) gives a declaration one level of visibility:
`export` makes it visible to modules that import the file. The build system adds
a second, above it:

| Level | Written | Visible to |
|---|---|---|
| Module | `export fn toCents(...)` in `cents.buri` | Any file **inside this library** that imports `//lib/money/cents` |
| Library | `from "//lib/money/cents.buri" export { Cents };` in `lib.buri` | Any target that declares this library in `dependencies` |

Nothing changes about `export` itself. A library-level export is a re-export
([`SPEC.md` §4.2.1](../SPEC.md)) written in a file the build system knows the
name of, and the rule is mechanical: **if it is not named in `lib.buri`, it is
not reachable from outside the library.**

```buri repo=cli/tests/example package=//lib/money
// lib/money/lib.buri — the surface of //lib/money, complete.

from "//lib/money/cents.buri" export { Cents, fromDollars, fromCents, add, isZero, format };
from "//lib/money/parse.buri" export { ParseError, parse };
```

```buri
// lib/money/cents.buri

export struct Cents(I64);          // name exported, contents not

export fn fromDollars(d: I64): Cents { Cents(d * 100) }
export fn fromCents(c: I64): Cents { Cents(c) }

// A method is declared inside an `impl` for its type, and that `impl` lives in
// the module that declares the type — which is what decides how a library's
// files are laid out.
impl Cents {
  export fn add(self, other: Cents): Cents { Cents(self.0 + other.0) }

  // Exported so `parse.buri` can reach it, since the field is module-private.
  // Absent from lib.buri, so it stops at the library boundary.
  export fn toCents(self): I64 { self.0 }
}
```

From `//cmd/server`:

```buri repo=cli/tests/example package=//cmd/server
from "//lib/money/lib.buri" import { Cents, format };        // fine
from "//lib/money/lib.buri" import { toCents };              // ERROR: "//lib/money" does not export `toCents`
from "//lib/money/cents.buri" import { toCents };        // ERROR: internal to //lib/money
```

The property this buys is that reviewing a library's API is reading one file.
Not grepping for `export` across forty, not consulting a generated manifest that
may be stale — opening `lib.buri`, which is the same file the compiler consults.

## Module paths

There are no relative imports ([`SPEC.md` §4.1.1](../SPEC.md)). Every module
path is absolute, so a path means the same module wherever it is written and a
file can move between directories without its own imports changing.

| Written | Resolves to | Legal from |
|---|---|---|
| `"core/list"` | A standard library module | Anywhere |
| `"//lib/money"` | `lib/money/lib.buri` — the library's surface | Anywhere the dependency is declared and visibility granted |
| `"//lib/money/cents"` | `lib/money/cents.buri` — one module inside it | Only from inside `//lib/money` |
| `"//lib/money/main"` | `lib/money/main.buri` — a binary's entry point | Only from that binary's own test sources |
| `"//lib/money/testing"` | `lib/money/testing/lib.buri` — the library's test utilities | Only from a test source, anywhere |
| `"core/testing/assert"` | The test platform | Only from a test source, anywhere |

So the same spelling, `//lib/money`, is a package path in a `BUILD.buri`, an
entry in `dependencies`, and a module path in an import — and it means the same
library each time. The rules the compiler enforces:

1. **A `//pkg` import requires a matching `dependencies` entry** in the
   importing target's rule, and visibility from the importing package. Both
   directions are errors: a use with no entry, and an entry nothing uses.
2. **A `//pkg/inner` import resolves only inside `//pkg`.** From outside, the
   diagnostic points at the library:

   ```
   error: //lib/ledger imports //lib/money/cents, which is internal to //lib/money
     --> lib/ledger/entry.buri:4:6
      |
    4 | from "//lib/money/cents.buri" import { Cents };
      |      ^^^^^^^^^^^^^^^^^^^
      |
      = import the library instead: from "//lib/money/lib.buri" import { Cents };
      = only names re-exported by lib/money/lib.buri are available
   ```
3. **A module path may not also be a package path.** If `lib/money/cents/` were
   a package, then `lib/money/cents.buri` in the parent would have two meanings;
   the build graph rejects the pair by name, and names both.
4. **`//pkg` is `lib.buri` and nothing else.** `//lib/money/lib` is not a legal
   path — one spelling per module.
5. **Rules inside a package do not reach into each other.** A binary imports the
   co-located library as `//pkg`, never `//pkg/render`; the library may not
   import `//pkg/main` at all.
6. **A path containing a `testing` segment is importable only from a test
   source.** See below.
7. **Circular imports are an error** at the module level already; package cycles
   are the same rule one level up.

## The `testing/` surface

A library often has code that exists only to make *other people's* tests
possible: a fake, a fixture, a builder, a matcher. It belongs with the library —
it changes when the library changes — and it must never reach a production
binary. Both follow from one rule:

> **A module path containing a `testing` segment may be imported only from a
> test source.**

That covers every case with no new field and nothing to remember to declare:

| Path | What it is |
|---|---|
| `//lib/ledger/testing` | A library's own test utilities, in `lib/ledger/testing/lib.buri` |
| `//lib/testing/fakes` | A standalone package of shared test infrastructure |
| `core/testing/assert` | The test platform |

The restriction is in the import line, where the person writing the import is
looking, rather than in a build file three directories away. A production module
that reaches for one gets:

```
error: lib/store/file_store.buri imports a test-only module
  --> lib/store/file_store.buri:6:6
   |
 6 | from "//lib/ledger/testing/lib.buri" import { sample };
   |      ^^^^^^^^^^^^^^^^^^^^^^
   |
   = a path containing `testing` may be imported only from a test source
   = lib/store/file_store.buri is in //lib/store's library sources
```

### A library's own `testing/`

```
lib/ledger/
  BUILD.buri
  lib.buri              <- //lib/ledger
  entry.buri
  testing/
    lib.buri            <- //lib/ledger/testing
    fixtures.buri
  test/
    ledger.buri
```

```textproto schema=build
library {
  sources: ["entry.buri", "posting/rules.buri"]
  dependencies: ["//lib/money"]

  testing {
    sources: ["testing/fixtures.buri"]
  }

  test {
    sources: ["test/ledger.buri"]
  }
}
```

`testing/lib.buri` is a second entry point of the same package, and behaves
exactly like the first one level down: it is the complete surface of
`//lib/ledger/testing`, made of re-exports, and a name absent from it is
unreachable.

```buri repo=cli/tests/example package=//lib/ledger role=test
// lib/ledger/testing/lib.buri — the surface of //lib/ledger/testing.
from "//lib/ledger/testing/fixtures.buri" export { sample, oneOff };
```

Four properties come from it being *in the package* rather than beside it:

- **It may import the library's internals.** `//lib/ledger/entry` is available
  to it, so a fake can be built out of the real thing without a back door in the
  public surface. This is the reason to prefer it over a separate package.
- **It has its own `dependencies`.** A fake usually needs less than the real
  implementation, and occasionally needs something else entirely; those entries
  do not become dependencies of the library.
- **It is never linked into a production artifact.** It compiles only into test
  binaries, so it is a leaf in every non-test build and costs nothing there.
- **It inherits the library's `visibility` and `tags`.** Being allowed to test
  against `//lib/ledger` and being allowed to depend on it are the same
  permission.

A consumer's test reaches it the way it reaches any library — declared, and by
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

The cost of the convention is that `testing` is a reserved directory name: a
package cannot have a product subdirectory called `testing`, and a package path
cannot contain that segment. That is the whole price, and it is visible the
moment you try.

## The re-export declaration

`lib.buri` is made of re-exports:

```buri repo=cli/tests/example package=//lib/money
from "//lib/money/cents.buri" export { Cents, fromCents };
from "//lib/money/cents.buri" export { add as addMoney };   // renaming is allowed
from "//lib/money/cents.buri" export *;                     // ERROR: expected `{`, found `*`
```

There is no `export *` for the same reason there is no bare `import *`: every
name that enters or leaves a module is written in that module's source. Here it
also means the surface cannot grow by accident — adding an `export` to an
internal module publishes nothing until someone edits `lib.buri`, and that edit
shows up in review as a change to the API, which is what it is.

A `lib.buri` may also declare things itself. It is an ordinary module that
happens to be the entry point, so this is fine:

```buri repo=cli/tests/example package=//lib/money
from "//lib/money/cents.buri" export { Cents, fromCents };
from "//lib/money/cents.buri" import { Cents, toCents };

/// Declared here rather than re-exported; both are public surface. A free
/// function, not a method: `Cents` is declared in cents.buri, and a method must
/// live in its type's defining module ([`SPEC.md` §6.7.3](../SPEC.md)).
export fn isRound(c: Cents): Bool { c.toCents() % 100 == 0 }
```

Three consequences worth stating outright:

- **Methods are filtered by the surface, like everything else.** Method calls
  resolve through the receiver's defining module rather than through scope
  ([`SPEC.md` §6.7](../SPEC.md)), which means a type could otherwise smuggle
  operations across the boundary: `from "//lib/money/lib.buri" import { Cents }` would
  make every method on `Cents` callable, including `toCents`, whether or not
  `lib.buri` mentioned it. So the rule is uniform — **a name is on the surface
  if `lib.buri` exports it, and a method call from outside the library resolves
  only to names on the surface.** Exporting `add` makes both `add(a, b)` and
  `a.add(b)` available; leaving out `toCents` removes both.

  Resolution itself does not change: still one type, one module, one lookup,
  with a visibility filter applied after it — which is exactly what `export`
  already does one level down, where a private method is not callable from
  another module either. The cost is that a library's methods have to be listed
  in `lib.buri` one at a time. They are also its API, so they were going to be
  listed anyway.

  Inside the library, [`SPEC.md` §4.1](../SPEC.md) applies unchanged: importing
  `Cents` from `//lib/money/cents` brings all of its exported methods,
  `toCents` included.

- **Member visibility and the library boundary are different mechanisms** that
  compose. `export` on a field hides a representation from every module
  including its own library's; the library boundary hides a name from every
  target but its own. `Cents` is both: internal code can construct one and
  cannot see inside it.

- **A method on an unexported type is unreachable**, which is the intended
  behavior and is also a lint (`dead-code`, which every repository runs).

One layout consequence, which surprises people once and then never again: a
type's methods must be declared in the module that declares the type, so `Cents`
and everything spelled `c.something()` live in one file, however long it gets.
Functions *over* a type go anywhere — including functions over `[Cents]`, which
can never be methods at all, since the defining module of `[T]` is `core/list`.
A library's file layout follows its types, not its verbs:

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

Within the library, `entry.buri` imports `//lib/ledger/posting/rules` and
`rules.buri` imports `//lib/ledger/entry` — absolute paths, no visibility rules,
no build graph. `lib.buri` re-exports from wherever the names live:

```buri repo=cli/tests/example package=//lib/ledger
// lib/ledger/lib.buri
from "//lib/ledger/entry.buri" export { Entry, entry, total };
from "//lib/ledger/posting/rules.buri" export { Rule, apply };
```

Subdirectory nesting has no cost in the build graph: one library is one compile
action set regardless of how its files are arranged. Split a directory into a
package when you want a **boundary** — a different visibility, a different tag,
a separate test suite, a cache edge that stops churn from propagating — and not
when a directory merely has a lot of files in it.

## The `test/` directory

`test/` is reserved. A `.buri` file under `test/` must appear in a rule's
`test.sources`, may not appear in `sources`, and may not be imported by
anything — including other test sources, which are compiled independently. See
[`testing.md`](./testing.md).
