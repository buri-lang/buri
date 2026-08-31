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
effect may be declared ahead of the runtime that will answer it, and then its
row names no platform at all, so dropping it from the context is the whole of
the fix. No effect is in that state today — every one `core/effect` declares is
granted somewhere — but the clause is what a reader meets on the day one is,
which is the point of writing it rather than an empty list.

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
against the signature it had been reviewed with simply started compiling.

An empty list is still not a promise that the list will fill, and `Listen` and
`Sockets` are what that looks like once the runtime arrives. Both landed
declared and granted by nobody; both are granted now on `LINUX` and `MACOS`,
because holding a port open is a native program's authority; and neither will
ever be granted on `JS` or `WEB`, because a page is served rather than serving
and its host has no way to accept a connection. Half of that row filled and the
other half never will. The row says who grants the effect now, and the reason
says why — nothing in it was ever a schedule.

## A program that provokes it

```buri fail code=host-not-granted platform=JS
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "ui/effect" import { Ui, Watch };

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        Ui: host.ui,
        Watch: host.watch,
    };
    let _ = io.println(ctx, "this program has no page to mount into").ignore();
    .Ok(())
}
```

The same source under `platform: WEB` compiles and mounts. `platform=JS` on the
fence is what tells the documentation harness which output to check it as —
without one a snippet is checked with the whole host granted, because it builds
no output.
