---
title: A suite's filesystem is written in the suite
message: '`test {{ data }}` is retired'
note: the field seeded an in-memory filesystem from files on disk, which only the JavaScript runner could be handed — a linked test binary has no runner, so `data()` was empty there and every read of a declared file answered differently on the two backends
fix: bind the files in the suite instead, as in `context {{ Fs: fs().files([("test/golden/statement.txt", "…")]) }}` from `core/host/testing`
reproduction: none
---
# A suite's filesystem is written in the suite

```text
error: `test { data }` is retired [retired-test-data]
```

## What to do

Delete the `data` entry, and give the suite its filesystem where the rest of its
context is written:

```buri role=test
# from "core/testing/assert" import * as assert;
# from "core/host/testing" import { alloc, fs as testFs };
# from "core/effect" import { Alloc, Fs };
# from "core/fs" import * as fs;
# fn render(): Str { "coffee  $4.50" }
test "renders the statement" {
  let ctx = context {
    Alloc: alloc(),
    Fs: testFs().files([("test/golden/statement.txt", "coffee  $4.50")]),
  };
  let want = assert.ok(fs.readText(ctx, "test/golden/statement.txt"));
  assert.eq(render(), want);
}
```

A golden that is read straight back is a golden the filesystem is not doing
anything for, and the shorter spelling of the same test is
`assert.eq(render(), "coffee  $4.50")`.

## Why

`data` named files on disk, and the *runner* read them and handed the suite
their contents. That made a suite's filesystem a fact about the build rather
than about the program, and it could only be told to a suite the toolchain ran
under a runner. A linked test binary has none: `data()` in a native test binary
was empty, so a package that declared `data` read `.Err(.NotFound)` where
`buri test` read the file. The toolchain hid that by sending every suite
declaring `data` back to JavaScript — one build-file field deciding which
backend a suite was allowed to run on, to protect an answer the two backends did
not agree about.

`fs().files([...])` is the same seeding written where it can be honest: it is
the suite's own text, both backends read it the same way, and nothing in the
build file decides what the program sees.
