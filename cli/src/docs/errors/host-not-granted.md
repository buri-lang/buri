---
title: A platform grants the effects its host exports
message: `{platform}` does not grant `{name}`
note: a platform is the set of effects its host exports; {because}
fix: drop `{effect}` from the context{elsewhere}
---
# A platform grants the effects its host exports

```text
error: `JS` does not grant `ui` [host-not-granted]
```

## What to do

Drop the effect from the context, or build this target for a platform that
grants it. Where the fix names no platform to build for, there is none: an
effect can be declared ahead of the runtime that answers it, and while it is,
dropping it from the context is the whole of the fix.

## Why

A platform *is* the set of effects its host exports, so a platform that does
not grant one does not export the name for it. Asking for it is then an
ordinary unresolved name at the line that asked, and there is no second
declaration anywhere that has to be kept in step.

The same sentence covers an effect *nobody* grants. A declaration may land
before its implementation — the signature is the expensive thing to change once
programs are written against it — and the table simply gives that effect an
empty list of platforms. No new mechanism, no "not implemented yet" flag: it is
withheld everywhere, for a stated reason, and granting it later is one row.

`Tasks` is the worked example, in both directions. It landed declared and
granted by nobody, was refused on all four platforms with that reason, and was
then granted on three by editing that one row — at which point programs written
against the signature it had been reviewed with simply started compiling. No row
is empty today; the case above is what the next such effect will use.

## A program that provokes it

```buri fail code=host-not-granted platform=JS
from "core/effect/lib.buri" import { Alloc, Stdout };
from "core/host/lib.buri" import * as host;
from "ui/effect/lib.buri" import { Ui, Watch };

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
