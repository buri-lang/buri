# Conformance is declared, never inferred

```text
error: `HostStdout` does not implement `Alloc` [missing-conformance]
```

## What to do

bind a value whose type has `impl Alloc for ...`; an effect is an ordinary interface, so a test double is a struct with those methods

## A program that provokes it

```buri fail code=missing-conformance
# from "core/effect" import { Alloc, Stdout };
# from "core/host" import * as host;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.stdout, Stdout: host.stdout };
  let _ = ctx.println("ready");
  .Ok(())
}
```
