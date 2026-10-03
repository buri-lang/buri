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

`buri lint --fix` and the editor's quick fix make the edit for you.

| what | written | who may write it |
|---|---|---|
| a **surface** — a library's `lib.buri`, or its `testing/lib.buri` | `"//lib/money"`, `"//lib/money/testing"`, `"core/list"` | anyone the dependency and visibility rules allow, including the package's own suite |
| a **file** inside a package | `"//lib/money/cents.buri"`, `"//cmd/app/main.buri"` | only another file of that same package |

The two look alike: `"//lib/money/testing"` and `"//lib/money/cents"` differ by
one segment. What's on disk decides, so the compiler works out the fix rather
than the path spelling it.

A binary's entry point is a file too: a package with only a binary has no
`lib.buri` for `//cmd/app` to name. Write `"//cmd/app/main.buri"`, from that
binary's own test sources only.

A path that names a file inside another package is `internal-import` instead.
