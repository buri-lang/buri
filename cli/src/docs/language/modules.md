## 4. Modules

A source file is a module, named by its path from the repository root. The build
system groups modules into **libraries** and **binaries**. Only the syntax is
here; which module may import which is in
[`cli/src/docs/reference/build/overview.md`](./cli/src/docs/reference/build/overview.md).

### 4.1 Imports

The module path comes **first**, before the specifier list:

```buri
from "core/effect" import { Alloc, Stdout };
from "core/fs" import { FsRead, FsWrite };
from "core/list" import * as list;
from "core/list" import { filter, map };
from "core/list" import { map as listMap };
```

That ordering serves tooling rather than prose. By the time you open the brace,
the compiler already knows which module you mean, so an editor can offer the
module's exports as completions. With the path last, you type the specifier list
blind and the editor checks it afterwards.

A namespace import **must** be named. The grammar cannot derive
`from "core/list" import *;` — the only wildcard form is `* as <name>`. So no
identifier enters a module's scope unless the importing file writes that
identifier, or the namespace holding it. You can resolve every unqualified name
in a module by reading that module alone, and adding an export to a library can
never shadow or collide with a name in code that imports it.

Import declarations are terminated with `;`. Circular imports are an error.

#### 4.1.1 Module paths

**A module path names a surface, or a file inside your own package.** Those are
two different kinds of thing, so they are spelled differently:

| What | Written | Who may write it |
|---|---|---|
| a **surface** — a library's `lib.buri`, or its `testing/lib.buri` | `"//lib/money"`, `"//lib/money/testing"`, `"core/list"` | anyone the visibility rules allow, including the package's own suite |
| a **file** inside a package | `"//lib/money/cents.buri"`, `"//cmd/app/main.buri"` | only another file of that same package |

A surface is the one thing a package publishes, so naming it names the package.
`"//lib/money"` is the label dependents declare in a `dependencies`, and it is
the path they write in an import — one string in the two namespaces that meet
there. The library's own test source writes it too, because a suite reaches its
library the way a dependent does. Everything else is one file among many, so the
path has to say *which*, and the only honest answer is the file's name.

| Form | Example | Names |
|---|---|---|
| Standard library | `"core/list"`, `"ui/signal"` | A module of the standard library, which ships with the compiler. Every one of them is a surface. |
| A package's surface | `"//lib/money"`, `"//lib/money/testing"` | That package's `lib.buri`, or its `testing/lib.buri`. |
| A file of your own package | `"//lib/money/cents.buri"`, `"//cmd/app/main.buri"` | A file of this package, by its path from the repository root. |

**You cannot tell the two apart by their shape.** `"//lib/money/testing"` and
`"//lib/money/cents"` are the same string with one segment changed, yet the first
is a surface and the second is a file with its name left off. What is on disk
decides. A path missing a file name is `import-path-without-a-file`, and the
diagnostic works out which file it meant, since no textual rule could. A path
that leaves the package and names a file inside it is `internal-import`, because
what it reaches for is not on the surface.

A binary's entry point is a file for the same reason a surface is not:
`"//cmd/app"` would name that package's `lib.buri`, and a package with only a
binary has not got one. So write `"//cmd/app/main.buri"`, and only that binary's
own test sources may write it.

One spelling is accepted and is still not the one to write.
`"//lib/money/lib.buri"` names the surface by the file it is, and resolves to the
same module, because a module has one identity however you reach it. It is the
long way round rather than a second module. Nothing in the toolchain writes it,
and nothing in this repository contains one.

The standard library owns two reserved roots. `core/` is the deliberately small
set of essentials: the types every program uses and the effects every platform
might grant. `ui/` is the reactivity and styling vocabulary — a different kind of
thing and a much larger surface, so it gets its own root rather than diluting
what `core/` means. Nothing a repository declares can collide with either, since
a repository path always begins `//`.

**There are no relative module paths.** `"./cents"` and `"../money"` are not
module paths, and a leading `.` in an import is an error. So a path means the
same module wherever you write it. You can move a file between directories
without rewriting the imports inside it, and a reader never has to know where a
file sits to know what it imports.

`"//lib/money"` names the *library* rooted at `lib/money` — its `lib.buri`, and
transitively only what that file exports. `"//lib/money/cents.buri"` names an
individual module inside it, which the build system permits only from within the
same library. Both are ordinary module paths to the compiler. The visibility
rules in
[`cli/src/docs/reference/build/libraries.md`](./cli/src/docs/reference/build/libraries.md)
enforce the distinction.

One path segment is reserved: **`testing`**. A module path containing it is
test-only, and only a test source may import it (Section 11.2). That covers
`"core/testing/assert"`, `"core/host/testing"`, a library's own
utilities-for-testing-it at `"//lib/money/testing"`, and a whole package of
shared fixtures at `"//lib/testing/fakes"` — one rule, visible in the import
line, with nothing to declare. The segment is a *directory* name.
`"//lib/money/testing.buri"` is a file called `testing` and is not test-only,
because the segment that would have made it so is a file name.

One module is reserved the other way. **`"core/host"`** holds the platform's
implementations of the effects it grants, and only the module that exports `main`
may import it (Section 10.3). The two restrictions have the same shape, and
between them they name every place in a program where authority can enter. They
stay separate, though: `"core/host/testing"` is that same surface for a test
source, and the `testing` segment alone governs it — the module that exports
`main` may not import it, and a test source may.

None of this applies to method calls. `sq.area()` resolves through the receiver's
type rather than through scope (Section 6.7.3), so a type's own operations reach
wherever a value of that type reaches, with no import and no chance of collision.
Import a type and its methods come with it.

### 4.2 Exports

A declaration is module-private unless prefixed with `export`.

```buri
fn helper(x: Int): Int {
    x * 2
} // private

export fn double(x: Int): Int {
    helper(x)
} // public
```

Struct fields carry their own `export`, so a struct's name and its
representation are exported separately:

```buri
export struct UserId(Str); // name public, contents private

export struct Meters(export F64); // both public
```

A struct with any unexported field cannot be constructed, destructured, or
exhaustively matched outside its module.

An enum is the unit of its own visibility: its variants and their payload
fields are exported exactly when it is, and a variant writes no `export` of its
own (Section 5.7).

A type alias is a name like any other. `export type TenantId = Str;` puts it on
the module's surface, where anyone may import and re-export it. The alias stays
transparent across the boundary: it expands in the module that declared it, so an
importer gets the type the declaration names (Section 5.9).

`impl` and `derive` declarations are never exported (Section 6.7.1).

### 4.2.1 Re-exports

A module may export a name it imported, with one declaration that mirrors
`import`:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
from "//lib/money/cents.buri" export { Cents, fromCents };

from "//lib/money/cents.buri" export { add as addMoney };
```

There is no `export *`, for the same reason there is no bare `import *`: a module
writes every name it publishes in its own source. Re-exporting a name does not
import it, so write both declarations if the module also uses it.

Re-export is what makes a library's `lib.buri` a complete public surface. It
lists the library's API in one file, and a name missing from it is unreachable
from outside the library, both as a function and as a method
([`cli/src/docs/reference/build/libraries.md`](./cli/src/docs/reference/build/libraries.md)).

### 4.3 Order

Declarations are visible throughout their module regardless of order. Mutual
recursion between top-level functions requires no forward declarations.

---
