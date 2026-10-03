---
title: Or-pattern alternatives bind the same names
message: or-pattern alternatives must bind the same names
fix: bind the same names in every alternative, or split this into separate arms
---
# Or-pattern alternatives bind the same names

```text
error: or-pattern alternatives must bind the same names [or-pattern-bindings]
```

```buri fail code=or-pattern-bindings
enum Either {
    Left(Int),
    Right(Int),
}

fn value(e: Either): Int {
    match (e) {
        .Left(x) | .Right(y) => x,
    }
}
```

An arm has one body, and it can use only the names every alternative binds, at
the same type.
