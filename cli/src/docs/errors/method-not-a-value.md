---
title: A method is not a value
message: `{name}` is a method, and a method is not a value
fix: call it on a receiver: `x.{name}()`; to pass it on, wrap it in a lambda: `fn(x) => x.{name}()`
---
# A method is not a value

```text
error: `area` is a method, and a method is not a value [method-not-a-value]
```

## What to do

Call it on a receiver — `x.area()` — or, to pass it on, wrap it in a lambda:
`fn(x) => x.area()`.

## Why

A method is resolved through its receiver's type rather than looked up in
scope, so `sq.area` on its own has nothing to evaluate to. The lambda is where
the receiver becomes an argument, which is what a function value needs.

## A program that provokes it

```buri fail code=method-not-a-value use=errors wrap=body
let sq = Square { side: 3 };
let f = sq.area;
let _ = ctx.println("${f()}");
```
