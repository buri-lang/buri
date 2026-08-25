---
title: A pattern binds each name once
message: "`{name}` is bound twice in this pattern"
note: a name a pattern binds is bound once; to require two positions to be equal, match one and test the other in a guard
fix: rename one of them, or bind one and compare the other in a guard
---
```buri fail code=duplicate-pattern-binding
fn sumPair(p: (Int, Int)): Int {
  match (p) {
    (a, a) => a,
  }
}
```
