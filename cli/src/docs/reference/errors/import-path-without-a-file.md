---
title: A module that is not a surface is named by its file
message: "{path}" names no file
note: a surface is named as a module and everything else is a file, so this path is that file's name — the one you would type into an editor, extension and all
fix: 'write the file name: `"{path}.buri"`'
reproduction: none
---
# A module that is not a surface is named by its file

```text
error: "//lib/money/cents" names no file [import-path-without-a-file]
 --> lib/money/parse.buri:5:6
  |
5 | from "//lib/money/cents" import { Cents };
  |      ^^^^^^^^^^^^^^^^^^^
  |
  = a surface is named as a module and everything else is a file, so this path
    is that file's name — the one you would type into an editor, extension and
    all
  = fix: write "//lib/money/cents.buri"
```

## What to do

Add the file name the path already meant. The compiler works out which file
that is and offers the edit, so `buri lint --fix` and the editor's quick fix
will make it for you.

## Why

There are two kinds of thing an import path can name, and they are spelled
differently because they are different:

| what | written | who may write it |
|---|---|---|
| a **surface** — a library's `lib.buri`, or its `testing/lib.buri` | `"//lib/money"`, `"//lib/money/testing"`, `"core/list"` | anyone the dependency and visibility rules allow, including the package's own suite |
| a **file** inside a package | `"//lib/money/cents.buri"`, `"//cmd/app/main.buri"` | only another file of that same package |

A surface is the one thing a package publishes, so naming it is naming the
package: `//lib/money` is both the label its dependents declare in a
`dependencies` and the path they import, and it is what the library's own test
source writes too. Everything else is one file among many, so the path has to
say *which*, and the only honest answer is the name of the file.

**The two cannot be told apart by their shape**, which is why the fix is
resolved and not spelled. `"//lib/money/testing"` and `"//lib/money/cents"` are
the same string with one segment changed; the first is a surface and correct,
the second is a file with its name left off. What decides it is
`lib/money/testing/lib.buri` and `lib/money/cents.buri` being on disk, and
only the resolver can look.

A binary's entry point is a file for the same reason: `//cmd/app` would be that
package's `lib.buri`, and a package with only a binary has not got one. So it
is `"//cmd/app/main.buri"`, from that binary's own test sources and nowhere
else.

A path that leaves the package and names a file inside it is a different error:
`internal-import`, because what it reaches for is not on the surface.
