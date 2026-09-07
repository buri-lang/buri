---
title: A platform grants the effects its host exports
message: '`{name}` implements {effect}, which is not allowed on {platforms}'
note: a platform is the set of effects its host exports; {because}
fix: drop {effect} from the context{elsewhere}
---
# A platform grants the effects its host exports

```text
error: `ui` implements `Ui`, which is not allowed on the JS platform [effect-not-on-platform]
```

## What to do

Drop the effect from the context, or build this target for a platform that
grants it. If the fix names no platform to build for, no platform grants the
effect yet, and dropping it is the whole fix.

The error lands on the import when the program imported the name directly, and
on the member reference when the host came in as `* as host` — a namespace
import names no effect, so there is nothing to refuse until the program reaches
for a member.

## Which platforms it is checked against

- **A build producing one output** is checked against that output, so
  `buri build --output=js` on a binary that also declares WEB compiles only the
  JS one. A snippet pinned with `platform=` on its fence works the same way.
- **A binary's entry point** — `main.buri`, the one module that may import
  `core/host` at all — is checked against every platform its `outputs` name,
  plus every platform its suite names in `test.platforms`. So a binary declaring
  `[MACOS, WEB]` and binding `FsRead: host.fs` is refused, naming WEB.
- **Every other module** is checked against the platforms **its own rule
  declared**. A rule that declared none is never checked, because a library that
  says nothing about `platforms` is platform-generic.

An **effect type** is never platform-bound. `from "core/fs" import { FsRead }`
is legal on every platform, a page included, and so is `core/host/testing`'s
double for every effect. A platform binds the **host** half — the value `main`
binds.

## Why

A platform *is* the set of effects its host exports, so a platform that does not
grant an effect exports no name for it. The check needs only the build file and
the import line, both of which the editor has, so the language server reports it
as you type. The refusals that stay late — `native-run-not-available`,
`cryptography-not-available`, `networking-not-available` — are about what *this
toolchain* was built with instead.

An effect nobody grants yet gets the same sentence, from an empty row in the
same table. `Listen` is granted on `LINUX` and `MACOS`, where holding a port
open is a native program's authority, and never will be on `JS` or `WEB`. A row
says who grants an effect now, not when the rest will fill — and it can widen
too: `Sockets` was granted with `Listen` and only with it, until
`WebSocketClient` let a page get a socket without accepting one.

`Tasks` shows the other direction. It landed granted by nobody, then on the
three platforms that are not a page, and now on all four — one edit to one row
each time, and nothing to change in a program already written against the
signature.

## A program that provokes it

```buri fail code=effect-not-on-platform platform=JS
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

The same source under `platform: WEB` compiles and mounts. `platform=JS` tells
the documentation harness which output to check the snippet as; without one it
checks with the whole host granted.
