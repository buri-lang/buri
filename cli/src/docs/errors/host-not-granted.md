---
title: A platform grants the effects its host exports
message: `{platform}` does not grant `{name}`
note: a platform is the set of effects its host exports; {because}
fix: drop `{effect}` from the context, or build this target for a platform that grants it: {platforms}
---
# A platform grants the effects its host exports

```text
error: `JS` does not grant `ui` [host-not-granted]
```

## What to do

Drop the effect from the context, or build this target for a platform that
grants it.

## Why

A platform *is* the set of effects its host exports, so a platform that does
not grant one does not export the name for it. Asking for it is then an
ordinary unresolved name at the line that asked, and there is no second
declaration anywhere that has to be kept in step.

## A program that provokes it

```buri fail code=host-not-granted platform=JS
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "ui/effect" import { Ui, Watch };

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Ui: host.ui,
    Watch: host.watch,
  };
  let _ = ctx.println("this program has no page to mount into");
  .Ok(())
}
```

The same source under `platform: WEB` compiles and mounts. `platform=JS` on the
fence is what tells the documentation harness which output to check it as —
without one a snippet is checked with the whole host granted, because it builds
no output.
