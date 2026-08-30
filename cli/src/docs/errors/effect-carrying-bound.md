---
title: A type that carries an effect satisfies no trait bound
message: "`{type}` carries an effect, so it does not satisfy `{trait}`"
note: a type is either part of the world or part of your data (SPEC 10.1), and that is what lets a lambda capture a `T: Ord` without laundering a context (SPEC 10.6)
fix: pass a type that holds no capability, or drop the `{trait}` bound
---

```buri fail code=effect-carrying-bound
# from "core/effect/lib.buri" import { Alloc, Stdout };
# from "core/host/lib.buri" import * as host;

struct Holder<C> { export inner: C }

impl<C> Eq for Holder<C> {
  fn eq(self, other: Holder<C>): Bool { true }
}

fn hide<T: Eq>(x: T): fn() => T {
  fn() => x
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let smuggler = hide(Holder { inner: ctx });
  let _ = smuggler().inner.println("laundered through Eq");
  .Ok(())
}
```
