---
title: A suite's filesystem is written in the suite
message: '`test {{ data }}` is retired'
note: the field seeded an in-memory filesystem from files on disk, which only the JavaScript runner could be handed — a linked test binary has no runner, so `data()` was empty there and every read of a declared file answered differently on the two backends
fix: bind the files in the suite instead, as in `context {{ FileSystemRead: fs().files([("test/golden/statement.txt", "…")]) }}` from `core/host/testing`
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
# from "core/effect" import { Allocator };
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead };
# from "core/host/testing" import { alloc, fs as testFs };
# from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn render(): Str {
#     "coffee  $4.50"
# }

test "renders the statement" {
    let ctx = context {
        Allocator: alloc(),
        FileSystemRead: testFs().files([("test/golden/statement.txt", "coffee  $4.50")]),
    };
    let at = path.of(ctx, "test/golden/statement.txt");
    let want = assert.ok(fs.readText(ctx, at));
    assert.equal(render(), want);
}
```

If the golden is read straight back, the filesystem is doing nothing for it. The
shorter spelling of the same test is `assert.equal(render(), "coffee  $4.50")`.

## Why

`data` named files on disk, and the *runner* read them and handed the suite
their contents. A linked test binary has no runner: `data()` there was empty, so
a package that declared `data` read `.Err(.NotFound)` where `buri test` read the
file. `fs().files([...])` is the same seeding written in the suite's own text,
where both backends read it the same way.
