---
title: A variant with a payload is matched with one
message: '`{name}` has a payload, so the pattern needs one'
---

```buri fail code=missing-payload-pattern
enum Shape {
    Circle(Int),
    Square,
}

fn go(s: Shape): Int {
    match (s) {
        .Circle => 1,
        .Square => 0,
    }
}
```
