---
title: A `match` covers every case
message: this `match` does not cover `{witness}`
label: not covered
---
# A `match` covers every case

```text
error: this `match` does not cover `.Empty` [match-not-exhaustive]
```

```buri fail code=match-not-exhaustive
enum Shape {
    Circle(Int),
    Square(Int),
    Empty,
}

fn describe(s: Shape): Int {
    match (s) {
        .Circle(r) => r,
        .Square(n) => n,
    }
}
```

Add an arm for the named case, or a `_` arm for everything left.

Exhaustiveness turns a new variant into a compile error everywhere that has to
handle it. A `_` arm opts that `match` out.
