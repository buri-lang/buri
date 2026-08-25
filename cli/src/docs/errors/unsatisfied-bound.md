---
title: A bound is satisfied by a declaration
message: "`{type}` does not satisfy `{trait}`"
---
```buri fail code=unsatisfied-bound
struct Point { export x: Int, export y: Int }

fn same<T: Eq>(a: T, b: T): Bool {
  a == b
}

fn check(p: Point): Bool {
  same(p, p)
}
```
