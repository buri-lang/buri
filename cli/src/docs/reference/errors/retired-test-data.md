---
title: A suite's filesystem is written in the suite
message: '`test {{ data }}` is retired'
note: the field seeded an in-memory filesystem from files on disk, which only the JavaScript runner could be handed — a linked test binary has no runner, so `data()` was empty there and every read of a declared file answered differently on the two backends
fix: bind the files in the suite instead, as in `context {{ FileSystemRead: fs().files([("test/golden/statement.txt", "…")]) }}` from `platform/effect/testing`
reproduction: none
---
# A suite's filesystem is written in the suite

```text
error: `test { data }` is retired [retired-test-data]
```

Delete the `data` entry and write the files into the suite's context:

```buri role=test
# from "core/fs" import * as fs;
# from "core/fs" import { FileSystemRead };
# from "core/path" import * as path;
# from "core/testing/assert" import * as assert;
# from "platform/effect" import { Allocator };
# from "platform/effect/testing" import { alloc, fs as testFs };

# fn render(): Str {
#     "coffee  $4.50"
# }

test "renders the statement" {
    let ctx = context {
        Allocator: alloc(),
        FileSystemRead: testFs().files([
            ("test/golden/statement.txt", "coffee  $4.50"),
        ]),
    };
    let at = path.of(ctx, "test/golden/statement.txt");
    let want = assert.ok(fs.readText(ctx, at));
    assert.equal(render(), want);
}
```

If the test only reads the golden straight back, skip the filesystem:
`assert.equal(render(), "coffee  $4.50")`.
