---
title: A tool exports the entry point each block declares
message: '`tool.buri` exports no `{entry}`'
note: a `{entry} {{}}` block declares the function of that name, and the toolchain calls it
fix: 'export `{entry}` from `tool.buri`, or delete the block'
reproduction: none
---
# A tool exports the entry point each block declares

```textproto schema=build
tool {
    check {}
}
```

```buri
// tool.buri
from "core/effect" import { Allocator };
from "core/tool" import { Checked, CheckRequest };

export fn check<C: Allocator>(ctx: C, request: CheckRequest<Str>): Checked {
    Checked { diagnostics: [], needs: [] }
}
```
