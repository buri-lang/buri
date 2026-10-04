---
title: An import names something its module exports
message: '"{path}" does not export `{name}`'
---
# An import names something its module exports

```text
error: "core/list" does not export `notAThing` [unknown-export]
```

```buri fail code=unknown-export
from "core/list" export { notAThing };
```

```buri fail code=unknown-export
from "core/fs" import * as fs;
from "core/fs" import { FileSystemWrite };
from "core/path" import * as path;
from "platform/effect" import { Allocator };

export fn appendWal<C: Allocator + FileSystemWrite>(ctx: C): Bool {
    fs.appendBytes(ctx, path.of(ctx, "wal"))
}
```

```buri fail code=unknown-export
from "core/math" import * as math;
from "platform/effect" import { Allocator };

export fn root<C: Allocator>(ctx: C): Float {
    math.sqrt(2.0)
}
```

A re-export names only what its module exports, so a library's surface is never
wider than the modules behind it. A namespace like `fs` stands for the module
its import named, and only that module's exports may follow the dot. This covers
types, bounds and `impl` heads too, such as `list.Vector<Int>` and
`<T: order.Comparable>`. If no import bound the namespace at all, the error is
`unknown-name` instead.
