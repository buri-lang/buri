---
title: Comparison operators do not chain
message: comparison operators are non-associative
note: write `a < b && b < c` rather than `a < b < c`
fix: write `a < b && b < c` rather than `a < b < c`
---
# Comparison operators do not chain

```text
error: comparison operators are non-associative [chained-comparison]
```

```buri fail code=chained-comparison
fn between(a: Int, b: Int, c: Int): Bool {
  a < b < c
}
```

Non-associativity is what lets `f<T>(x)` read as a call: `(f < T) > (x)` is
not a program either, so no source has two readings.
