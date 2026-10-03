---
title: A `let` pattern must match every value
message: this pattern does not match every value of its type
note: a `let` binds unconditionally, so its pattern has to be irrefutable
fix: use `match`, which makes you say what the other cases do
---
# A `let` pattern must match every value

```text
error: this pattern does not match every value of its type [refutable-pattern]
```

```buri fail code=refutable-pattern
fn unwrap(o: Option<Int>): Int {
    let .Some(n) = o;
    n
}
```

Use `match`. No exception is thrown when the value doesn't fit, so you write out
the other cases.
