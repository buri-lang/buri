---
title: A namespace member is named by the module that exports it
message: "{path}" has no member `{name}`
note: the module exports {exports}
fix: correct the member name, or check `buri docs {path}`
---
# A namespace member is named by the module that exports it

```text
error: "core/fs" has no member `appendBytes` [no-such-member]
```

```buri fail code=no-such-member
from "core/fs" import * as fs;
from "core/fs" import { FileSystemWrite };
from "core/path" import * as path;
from "platform/effect" import { Allocator };

export fn appendWal<C: Allocator + FileSystemWrite>(ctx: C): Bool {
    fs.appendBytes(ctx, path.of(ctx, "wal"))
}
```

```buri fail code=no-such-member
from "core/math" import * as math;
from "platform/effect" import { Allocator };

export fn root<C: Allocator>(ctx: C): Float {
    math.sqrt(2.0)
}
```

A namespace like `fs` stands for the module its import named, and only that
module's exports may follow the dot. This covers types, bounds and `impl` heads
too, such as `list.Vector<Int>` and `<T: order.Comparable>`. If no import bound
the namespace at all, the error is `unresolved-name` instead.
