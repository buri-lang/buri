---
title: A payload pattern matches the values the variant holds
message: '`{name}` holds {expected} values, but {matched} were matched'
fix: match exactly {expected}, or end the pattern with `..`
---

```buri fail code=wrong-matched-value-count
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
