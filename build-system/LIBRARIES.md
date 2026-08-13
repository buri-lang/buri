# Libraries: `lib.buri` and the public surface

A library is a package with a `lib.buri`. That file is the library's entire
public surface — everything a dependent can import, and nothing else.

## Two levels of export

[`SPEC.md` §4.2](../SPEC.md) gives a declaration one level of visibility:
`export` makes it visible to modules that import the file. The build system adds
a second, above it:

| Level | Written | Visible to |
|---|---|---|
| Module | `export fn toCents(...)` in `cents.buri` | Any file **inside this library** that imports `./cents` |
| Library | `from "./cents" export { Cents };` in `lib.buri` | Any target that declares this library in `deps` |

Nothing changes about `export` itself. A library-level export is just a
re-export written in a file the build system knows the name of, and the rule is
mechanical: **if it is not reachable from `lib.buri`, it is not reachable from
outside the library.**

```buri
// lib/money/lib.buri — the surface of //lib/money, complete.

from "./cents" export { Cents, fromDollars, fromCents, add };
from "./format" export { format };
```

```buri
// lib/money/cents.buri

export opaque struct Cents(I64);

export fn fromDollars(d: I64): Cents { Cents(d * 100) }
export fn fromCents(c: I64): Cents { Cents(c) }
export fn add(self: Cents, other: Cents): Cents { Cents(self.0 + other.0) }

// Exported so `format.buri` can reach it, since `Cents` is opaque and `.0` is
// not visible outside this module. Absent from lib.buri, so it stops at the
// library boundary.
export fn toCents(self: Cents): I64 { self.0 }
```

From `//cmd/server`:

```buri
from "//lib/money" import { Cents, format };   // fine
from "//lib/money" import { toCents };         // ERROR: not exported by //lib/money
from "//lib/money/cents" import { toCents };   // ERROR: not a package
```

The property this buys is that reviewing a library's API is reading one file.
Not grepping for `export` across forty, not consulting a generated manifest that
may be stale — opening `lib.buri`, which is the same file the compiler consults.

## The re-export declaration

The grammar gains one production, symmetric with `Import` and with the same
path-first ordering and the same ban on unnamed wildcards:

```ebnf
Item            ::= Import
                  | ReExport
                  | Declaration

ReExport        ::= "from" STRING "export" "{" ImportSpecs? "}" ";"
```

```buri
from "./cents" export { Cents, fromCents };
from "./cents" export { add as addMoney };        // renaming is allowed
from "./cents" export *;                          // NOT derivable from the grammar
```

There is no `export *` for the same reason there is no bare `import *`
([`SPEC.md` §4.1](../SPEC.md)): every name that enters a scope should be written
in the file that puts it there. Here it also means the surface cannot grow by
accident — adding an `export` to an internal module publishes nothing until
someone edits `lib.buri`, and that edit shows up in review as a change to the
API, which is what it is.

A `lib.buri` may also declare things itself. It is an ordinary module that
happens to be the entry point, so this is fine:

```buri
from "./cents" export { Cents, fromCents };
from "./cents" import { Cents as C, toCents };

/// Declared here rather than re-exported. Both forms are public surface.
export fn isZero(self: C): Bool { self.toCents() == 0 }
```

Whether that should be allowed is [open question 3](./README.md#open-questions).

Three consequences worth stating outright:

- **Methods are filtered by the surface, like everything else.** Method calls
  resolve through the receiver's defining module rather than through scope
  ([`SPEC.md` §6.7](../SPEC.md)), which means a type could otherwise smuggle
  operations across the boundary: `from "//lib/money" import { Cents }` would
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
  `Cents` from `./cents` brings all of its exported methods, `toCents` included.

- **`opaque` and the library boundary are different mechanisms** that compose.
  `opaque` hides a representation from every module including its own library's;
  the library boundary hides a name from every target but its own. `Cents` is
  both: internal code can construct one and cannot see inside it.
- **A method on an unexported type is unreachable**, which is the intended
  behavior and is also a lint (`unreachable-export`, on by default).

## Import resolution

Every import string falls into exactly one of four forms, distinguishable
lexically, with no search path and no ambient resolution:

| Form | Example | Resolves to |
|---|---|---|
| Standard library | `"core/list"` | The toolchain's `core`. Never declared in `deps`. |
| Repository-absolute | `"//lib/money"` | The `lib.buri` of the library in that package. |
| Relative | `"./format"`, `"../posting/rules"` | A file in the **same package**, resolved against the importing file's directory. |
| External | `"@proto//well_known"` | Reserved. Not implemented. |

Rules the compiler enforces:

1. **A `//` import requires a matching `deps` entry** in the importing target's
   rule. Both directions are errors: an import without a dep, and a dep nothing
   imports.
2. **A `//` import names a package, never a file.** `"//lib/money/format"` does
   not resolve even though the file exists — packages are the unit of naming
   across a boundary.
3. **A relative import may not leave the package.** `from "../money/cents"` in
   `lib/ledger` is an error even though the path exists on disk, and the
   diagnostic points at `//lib/money` as the way to say it. This is the rule that
   makes `lib.buri` a real boundary rather than a convention.
4. **A relative import may not cross rules within a package.** In a package with
   both a library and a binary, `main.buri` reaches the library through
   `./lib` — the entry point — and never through `./render`.
5. **Circular imports are an error** at the module level already; package cycles
   are the same rule one level up.

```
error: lib/ledger/entry.buri imports across a package boundary
  --> lib/ledger/entry.buri:4:6
   |
 4 | from "../money/cents" import { Cents };
   |      ^^^^^^^^^^^^^^^^
   |
   = ../money/cents is in package //lib/money
   = import the library instead: from "//lib/money" import { Cents };
   = only names re-exported by lib/money/lib.buri are available
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

```textproto
library {
  name: "ledger"
  srcs: [
    "entry.buri",
    "posting/interest.buri",
    "posting/rules.buri",
  ]
  deps: ["//lib/money"]
  visibility: ["//cmd/...", "//lib/store"]

  test {
    srcs: ["test/ledger.buri"]
  }
}
```

Within the library, `entry.buri` imports `./posting/rules` and `rules.buri`
imports `../entry` — ordinary relative paths, no visibility rules, no build
graph. `lib.buri` re-exports from wherever the names live:

```buri
// lib/ledger/lib.buri
from "./entry" export { Entry, post, balance };
from "./posting/rules" export { Rule, Ruleset, apply };
```

Subdirectory nesting has no cost in the build graph: one library is one compile
action set regardless of how its files are arranged. Split a directory into a
package when you want a **boundary** — a different visibility, a different tag,
a separate test suite, a cache edge that stops churn from propagating — and not
when a directory merely has a lot of files in it.

## The `test/` directory

`test/` is reserved. A `.buri` file under `test/` must appear in a rule's
`test.srcs`, may not appear in `srcs`, and may not be imported by anything —
including other test files, which are compiled independently. See
[`TESTING.md`](./TESTING.md).
