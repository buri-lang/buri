---
title: A struct pattern mentions every field
message: this pattern does not mention {fields}
fix: match {fields} too, or end the pattern with `..` to ignore the rest
---
# A struct pattern mentions every field

```text
error: this pattern does not mention `y` [missing-field-pattern]
```

```buri fail code=missing-field-pattern
struct Point {
    export x: Int,
    export y: Int,
}

fn xOf(p: Point): Int {
    let Point { x } = p;
    x
}
```

Adding a field should break every pattern that takes the type apart. `..` is
how a pattern opts out.
