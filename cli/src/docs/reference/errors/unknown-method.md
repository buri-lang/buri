---
title: A method is one its type declares
message: `{type}` has no method `{method}`
fix: check the spelling, or declare it in `impl {type} {{ ... }}` in that type's own module
---
# A method is one its type declares

```text
error: `Square` has no method `area` [unknown-method]
```

```buri fail code=unknown-method use=errors wrap=body
let _ = io.println(ctx, "${Square { side: 3 }.perimeter()}").ignore();
```

```buri fail code=unknown-method use=errors wrap=body
let greeting = "hello";
let _ = io.println(ctx, "${greeting.len()}").ignore();
```

Methods live only in the module that declares the receiver's type. Nothing can
add a method from outside, so for a toolchain type the fix points at its page
instead of an `impl` block you couldn't write.
