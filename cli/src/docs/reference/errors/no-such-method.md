---
title: A method is looked up in its type's defining module
message: `{type}` has no method `{method}`
fix: check the spelling, or declare it in `impl {type} {{ ... }}` in that type's own module
---
# A method is looked up in its type's defining module

```text
error: `Square` has no method `area` [no-such-method]
```

## Why

A method is looked up in exactly one place: the module that declares the
receiver's type. There is no extension mechanism, so nobody can add a method to
a type from outside — which is why the fix points a toolchain type at its page
rather than at an `impl` block you could not write.

## A program that provokes it

```buri fail code=no-such-method use=errors wrap=body
let _ = io.println(ctx, "${Square { side: 3 }.perimeter()}").ignore();
```

```buri fail code=no-such-method use=errors wrap=body
let greeting = "hello";
let _ = io.println(ctx, "${greeting.len()}").ignore();
```
