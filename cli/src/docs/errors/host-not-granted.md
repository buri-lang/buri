# A platform grants the effects its host exports

```text
error: `JS` does not grant `ui` [host-not-granted]
```

## What to do

drop the effect from the context, or build this target for a platform that grants it

## Why

a platform *is* the set of effects its host exports, so a platform that does not grant one does not export the name for it, and there is no second declaration to keep in step

## The rule

`core/host` is the platform's implementations of the effects it grants, and it is
importable only from the module that exports `main` (SPEC rule 34). What it
exports is decided by the platform of the **output being built**, so the same
`main` may compile for one of a binary's outputs and not for another:

| Effect | LINUX | MACOS | JS | WEB |
|---|---|---|---|---|
| `Alloc`, `Stdout`, `Stderr`, `Clock`, `Rand` | yes | yes | yes | yes |
| `Fs`, `Net`, `Stdin`, `Env`, `Proc` | yes | yes | yes | no |
| `Ui`, `Watch`, `Fetch` | no | no | no | yes |

A page has no filesystem, no socket that does not freeze it — `Net.fetch` blocks
until the response arrives, so `WEB` grants `Fetch` instead — no standard input,
no command line and no process to exit. Nothing but a page has a document, so
nothing but a page has a reactive graph to grant.

Both halves of a grant are withheld together: the implementation struct as well
as the value. A host struct has no private field, so exporting `HostNet` while
withholding `net` would leave the authority one `Net: host.HostNet {}` away.

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

Compiled by the test suite, which checks that it still produces `host-not-granted` — so
this page cannot describe an error the compiler has stopped emitting.
