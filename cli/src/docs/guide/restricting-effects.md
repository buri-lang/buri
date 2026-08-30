## Restricting what propagates

Because effects are bounds, giving a callee less is just naming fewer of
them. It receives the same value and cannot use — or pass on — anything its
bounds omit:

```buri
# from "core/effect/lib.buri" import { Alloc, Fs, Stdout };
# from "core/host/lib.buri" import * as host;
fn logOnly<C: Stdout>(ctx: C, msg: Str): () {
  let _ = ctx.println(msg);
  let _f = ctx.readFile("/etc/passwd");  // ERROR: `C` has no method `readFile`
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Fs: host.fs };
  let _ = logOnly(ctx, "starting");    // same value, confined by its bound
  .Ok(())
}
```

No copy, no wrapper, no runtime cost, and confinement is transitive — `C` is
opaque at every downstream call site. When you want the value itself to lack the
effect rather than merely be unable to name it, wrap the context in a type
that satisfies fewer traits ([SPEC.md §10.8](../SPEC.md)).

One more thing falls out of effects being ordinary interfaces: **a test double
is a struct with methods.** A test builds a context the same way `main` does and
binds whichever implementations it wants, so there is no mocking framework and
the call site does not change. [`build/testing.md`](../build/testing.md#the-runners-context)
has the runner's own implementations and a worked fake.
