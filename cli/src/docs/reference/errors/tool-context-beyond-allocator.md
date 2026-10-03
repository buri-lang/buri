---
title: A tool's entry point is handed an allocator and nothing else
message: '`{entry}` asks for `{effect}`, and `ctx` in a tool has `Allocator` only'
note: what a tool answers is a function of what it was handed, so it cannot read the clock, the disk or the network
fix: 'bound `ctx` by `Allocator` alone; a file the tool needs to read goes in `needs`'
reproduction: none
---
# A tool's entry point is handed an allocator and nothing else

```buri
from "core/tool" import { Checked, CheckRequest };
from "platform/effect" import { Allocator };

export fn check<C: Allocator>(ctx: C, request: CheckRequest<Str>): Checked {
    Checked { diagnostics: [], needs: [] }
}
```

The build caches a tool's answer under its inputs. A tool that read anything
else could give two answers to one question, and the cache would serve the
wrong one.
