---
title: A method is looked up in its type's defining module
message: `{type}` has no method `{method}`
fix: check the spelling, or declare it in `impl {type} {{ ... }}` in that type's own module
---
# A method is looked up in its type's defining module

```text
error: `Square` has no method `area` [no-such-method]
```

## What to do

Check the spelling. If the type is one of yours, declare the method in an `impl`
block in that type's own module. If the type ships with the toolchain — a
`Result`, an `I64` — or belongs to another package, `buri docs <module>` lists
the methods it has.

Where there is a near miss the fix names it: "if you meant `mapErrCtx`, use
that; if not, `buri docs core/result` lists every method `Result<I64, Str>`
has". A method the standard library renamed is not a guess and gets the answer
instead — `len` was renamed to `length`.

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
