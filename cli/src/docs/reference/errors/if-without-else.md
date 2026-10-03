---
title: `if` is an expression, so it needs an `else`
message: `if` requires an `else` branch
note: `if` is an expression, so both branches must produce a value of the same type
fix: add `else {{ ... }}`; an `if` is an expression, so it has a value either way
---
# `if` is an expression, so it needs an `else`

```text
error: `if` requires an `else` branch [if-without-else]
```

```buri fail code=if-without-else
fn sign(n: Int): Int {
  let label = if (n > 0) { 1 };
  label
}
```

Without an `else`, the language would have to invent a value for the other
path.

This error means the `else` really is absent. A branch missing its `}`, or a
stray token between `}` and `else`, gets its own error instead.
