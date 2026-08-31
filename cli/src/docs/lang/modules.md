## 4. Modules

A source file is a module, named by its path from the repository root. Modules
are grouped into **libraries** and **binaries** by the build system; the rules
for which module may import which are in
[`cli/src/docs/build/overview.md`](./cli/src/docs/build/overview.md), and only
the syntax is here.

### 4.1 Imports

The module path comes **first**, before the specifier list:

```buri
from "core/list/lib.buri" import { map, filter };
from "core/list/lib.buri" import { map as listMap };
from "core/list/lib.buri" import * as list;
from "core/effect/lib.buri" import { Alloc, Fs, Stdout };
```

The ordering is chosen for tooling rather than for prose: by the time you open
the brace, the compiler already knows which module you mean, so an editor can
offer the module's exports as completions. With the path last, the specifier
list has to be typed blind and then retro-checked.

A namespace import **must** be named. `from "core/list/lib.buri" import *;` is not
derivable from the grammar — the only wildcard form is `* as <name>`. There is
consequently no way for an identifier to enter a module's scope without that
identifier, or the namespace holding it, being written in the importing file.
Every unqualified name in a module can be resolved by reading that module alone,
and adding an export to a library can never shadow or collide with a name in
code that imports it.

Import declarations are terminated with `;`. Circular imports are an error.

#### 4.1.1 Module paths

**A module path names a file.** It is a root, and then the path of a file
under it — the path you would type into an editor, extension and all.

| Form | Example | Names |
|---|---|---|
| Standard library | `"core/list/lib.buri"`, `"ui/signal/lib.buri"` | A module of the standard library, which ships with the compiler. |
| Repository-absolute | `"//lib/money/lib.buri"`, `"//lib/money/cents.buri"` | A file of this repository, by its path from the root. |

A path used to be a name with the file left off, and the rule connecting the
two was something you had to know: a library was its *directory*, a module
inside one dropped its extension, a binary's entry point was the word `main`,
and a testing surface was a directory that meant a file two levels down. Four
spellings for four kinds of the same thing, and a module with two names.

Now there is one name. `//` is the repository root, `core/` and `ui/` are the
standard library, and everything after that is a file:

| What it is | Path |
|---|---|
| A library's surface | `"//lib/money/lib.buri"` |
| A module inside a library | `"//lib/money/cents.buri"` |
| A library's testing surface | `"//lib/money/testing/lib.buri"` |
| A binary's entry point | `"//cmd/server/main.buri"` |
| A schema | `"//proto/address.proto"` |

A path that names no file — `"//lib/money"`, `"core/list"` — is
`import-path-without-a-file`, and inside a repository the diagnostic works out
which file the old spelling meant and offers the edit.

The standard library owns two reserved roots. `core/` is the deliberately small
set of essentials — the types every program uses and the effects every platform
might grant. `ui/` is the reactivity and styling vocabulary, which is a
different kind of thing and a much larger surface, so it has its own root
rather than diluting what `core/` means. Both are reserved: a repository path
always begins `//`, so nothing a repository declares can collide with either.

**There are no relative module paths.** `"./cents"` and `"../money"` are not
module paths, and a leading `.` in an import is an error. A path therefore
means the same module wherever it is written, a file can be moved between
directories without rewriting the imports inside it, and a reader never has to
know where a file sits to know what it imports.

`"//lib/money/lib.buri"` names the *library* rooted at `lib/money` — that file,
and transitively only what it exports. `"//lib/money/cents.buri"` names an
individual module inside it, which the build system permits only from within
the same library. Both are ordinary module paths to the compiler; the
distinction is enforced with the visibility rules in
[`cli/src/docs/build/libraries.md`](./cli/src/docs/build/libraries.md).

One path segment is reserved: **`testing`**. A module path containing it is
test-only, and may be imported only from a test source (Section 11.2). That
covers `"core/testing/assert/lib.buri"`, `"core/host/testing/lib.buri"`, a
library's own utilities-for-testing-it at `"//lib/money/testing/lib.buri"`, and
a whole package of shared fixtures at `"//lib/testing/fakes/lib.buri"` — one
rule, visible in the import line, with nothing to declare. The segment is a
*directory* name: `"//lib/money/testing.buri"` is a module called `testing` and
is not test-only, because the segment that would have made it so is a file
name.

One module is reserved the other way: **`"core/host/lib.buri"`**, the platform's
implementations of the effects it grants, is importable only from the module
that exports `main` (Section 10.3). The two restrictions are the same shape, and
between them they name every place in a program where authority can enter. They
are also separate: `"core/host/testing/lib.buri"` is the same surface for a test
source, and it is governed by the `testing` segment alone — the module that
exports `main` may not import it, and a test source may.

None of this applies to method calls. `sq.area()` resolves through the receiver's
type rather than through scope (Section 6.7.3), so a type's own operations are
available wherever a value of that type is, with no import and no possibility of
collision. Importing a type brings its methods with it.

### 4.2 Exports

A declaration is module-private unless prefixed with `export`.

```buri
fn helper(x: Int): Int { x * 2 }           // private
export fn double(x: Int): Int { helper(x) } // public
```

Struct fields carry their own `export`, so a struct's name and its
representation are exported separately:

```buri
export struct UserId(Str);          // name public, contents private
export struct Meters(export F64);   // both public
```

A struct with any unexported field cannot be constructed, destructured, or
exhaustively matched outside its module.

An enum is the unit of its own visibility: its variants and their payload
fields are exported exactly when it is, and a variant writes no `export` of its
own (Section 5.7).

A type alias is a name like any other: `export type TenantId = Str;` puts it on
the module's surface, where it can be imported and re-exported. The alias stays
transparent across the boundary — it expands in the module that declared it,
so what an importer gets is the type the declaration names (Section 5.9).

`impl` and `derive` declarations are never exported (Section 6.7.1).

### 4.2.1 Re-exports

A module may export a name it imported, in one declaration that mirrors
`import`:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
from "//lib/money/cents.buri" export { Cents, fromCents };
from "//lib/money/cents.buri" export { add as addMoney };
```

There is no `export *`, for the same reason there is no bare `import *`: every
name a module publishes is written in that module's own source. Re-exporting a
name does not import it — write both declarations if the module also uses it.

Re-export is what makes a library's `lib.buri` a complete public surface: it
lists the library's API in one file, and a name absent from it is unreachable
from outside the library, as a function and as a method
([`cli/src/docs/build/libraries.md`](./cli/src/docs/build/libraries.md)).

### 4.3 Order

Declarations are visible throughout their module regardless of order. Mutual
recursion between top-level functions requires no forward declarations.

---
