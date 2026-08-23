## 4. Modules

A source file is a module, named by its path from the repository root. Modules
are grouped into **libraries** and **binaries** by the build system; the rules
for which module may import which are in
[`cli/src/docs/`](./cli/src/docs/build/overview.md), and only the syntax is here.

### 4.1 Imports

The module path comes **first**, before the specifier list:

```buri
from "core/list" import { map, filter };
from "core/list" import { map as listMap };
from "core/list" import * as list;
from "core/cap" import { Alloc, Fs, Stdout };
```

The ordering is chosen for tooling rather than for prose: by the time you open
the brace, the compiler already knows which module you mean, so an editor can
offer the module's exports as completions. With the path last, the specifier
list has to be typed blind and then retro-checked.

A namespace import **must** be named. `from "core/list" import *;` is not
derivable from the grammar — the only wildcard form is `* as <name>`. There is
consequently no way for an identifier to enter a module's scope without that
identifier, or the namespace holding it, being written in the importing file.
Every unqualified name in a module can be resolved by reading that module alone,
and adding an export to a library can never shadow or collide with a name in
code that imports it.

Import declarations are terminated with `;`. Circular imports are an error.

#### 4.1.1 Module paths

A module path is one of two forms, told apart by their first characters:

| Form | Example | Names |
|---|---|---|
| Standard library | `"core/list"`, `"ui/signal"` | A module of the standard library, which ships with the compiler. |
| Repository-absolute | `"//lib/money"`, `"//lib/money/cents"` | A module of this repository, by its path from the root. |

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

`"//lib/money"` names the *library* rooted at `lib/money` — its `lib.buri`, and
transitively only what that file exports. `"//lib/money/cents"` names an
individual module inside it, which the build system permits only from within
the same library. Both are ordinary module paths to the compiler; the
distinction is enforced with the visibility rules in
[`cli/src/docs/build/libraries.md`](./cli/src/docs/build/libraries.md).

One path segment is reserved: **`testing`**. A module path containing it is
test-only, and may be imported only from a test source (Section 11.2). That
covers `"core/testing/assert"`, `"core/testing/context"`, a library's own
utilities-for-testing-it at `"//lib/money/testing"`, and a whole package of
shared fixtures at `"//lib/testing/fakes"` — one rule, visible in the import
line, with nothing to declare.

One module is reserved the other way: **`"core/host"`**, the platform's
implementations of the effects it grants, is importable only from the module
that exports `main` (Section 10.3). The two restrictions are the same shape, and
between them they name every place in a program where authority can enter.

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

Struct fields and enum variants carry their own `export`, so a type's name and
its representation are exported separately:

```buri
export struct UserId(Str);          // name public, contents private
export struct Meters(export F64);   // both public
```

A type with any unexported field or variant cannot be constructed, destructured,
or exhaustively matched outside its module.

`impl` and `derive` declarations are never exported: whether a type satisfies a
trait is a property of the type, visible wherever the type is.

### 4.2.1 Re-exports

A module may export a name it imported, in one declaration that mirrors
`import`:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
from "//lib/money/cents" export { Cents, fromCents };
from "//lib/money/cents" export { add as addMoney };
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
