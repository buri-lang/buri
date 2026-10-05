---
title: A payload pattern matches the values the variant holds
message: '`{name}` holds {expected}, but the pattern matches {matched}'
fix: match exactly {expected}, with `_` for any the arm does not need
---

```buri fail code=payload-pattern-count
enum Shape {
    Circle(Int, Int),
    Square,
}

fn go(s: Shape): Int {
    match (s) {
        .Circle(a) => a,
        .Square => 0,
    }
}
```
