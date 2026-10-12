---
title: A member is read from a value of known type
message: '`.{member}` needs a value whose type is known'
fix: annotate the value, as in `fn(q: Str) => ...` for a closure's parameter
---

```buri fail code=untyped-receiver
fn go(): Int {
    let size = fn(q) => q.length();
    size("four")
}
```

Write the parameter's type: `fn(q: Str) => q.length()`.

A method or field belongs to a type, so the checker needs the type before it
can look one up. It reads types left to right, and a closure with no expected
function type gives its parameters none until later code uses them, which is
too late.
