---
title: A pattern matches the shape of the scrutinee
message: 'expected `{expected}`, found a `{found}` pattern'
---

```buri fail code=pattern-type-mismatch
struct Point { export x: Int }

fn go(n: Int): Int {
  match (n) {
    Point { x } => x,
    _ => 0,
  }
}
```
