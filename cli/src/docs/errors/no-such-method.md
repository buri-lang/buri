---
title: A method is looked up in its type's defining module
message: `{type}` has no method `{method}`
fix: check the spelling, or declare it in `impl {type} {{ ... }}` in that type's own module — a method may not be added from anywhere else
---
# A method is looked up in its type's defining module

```text
error: `Square` has no method `area` [no-such-method]
```

## What to do

Check the spelling. If the type is one of yours, declare the method in an `impl`
block in that type's own module. If the type ships with the toolchain — a
`Result`, an `I64` — or belongs to another package, it cannot gain one from
here, and `buri docs <module>` lists the methods it has.

## Why

A method is looked up in exactly one place: the module that declares the
receiver's type. There is no extension mechanism, so a method cannot be added
to a type from outside — which is also what makes resolution a single lookup
rather than a search.

## A program that provokes it

```buri fail code=no-such-method use=errors wrap=body
let _ = ctx.println("${Square { side: 3 }.perimeter()}");
```
