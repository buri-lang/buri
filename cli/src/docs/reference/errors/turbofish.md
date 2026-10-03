---
title: Type arguments are written without `::`
message: type arguments in an expression are written without `::`
note: `::` was needed when `<` in expression position was always a comparison; it no longer is
fix: remove the `::`, as in `list.empty<Int>()`
---
# Type arguments are written without `::`

```text
error: type arguments in an expression are written without `::` [turbofish]
```

```buri fail code=turbofish
# from "core/list" import * as list;
fn empty(): [Int] {
  list.empty::<Int>()
}
```

`buri lint --fix` and an editor's quick fix remove the `::` for you.

Other languages need `::` to tell `f<A>(x)` from `(f < A) > (x)`. Buri's
comparison operators don't chain, so the second reading isn't a program anyway.
