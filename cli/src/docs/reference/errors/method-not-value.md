---
title: A method is not a value
message: `{name}` is a method, and a method is not a value
fix: call it on a receiver: `x.{name}()`; to pass it on, wrap it in a lambda: `fn(x) => x.{name}()`
---
# A method is not a value

```text
error: `area` is a method, and a method is not a value [method-not-value]
```

```buri fail code=method-not-value use=errors wrap=body
let sq = Square { side: 3 };
let f = sq.area;
let _ = io.println(ctx, "${f()}").ignore();
```

A method is found through its receiver's type, not looked up in scope, so
`sq.area` alone has nothing to evaluate to. The lambda turns the receiver into
an argument.
