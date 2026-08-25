---
title: A field is named by the type that declares it
message: `{type}` has no field `{field}`
fix: check the spelling, or name a field the type declares
---
# A field is named by the type that declares it

```text
error: `Rec` has no field `f1` [no-such-field]
```

## What to do

Correct the spelling, or name a field the type declares.

## Why

There is no structural typing and no inheritance, so a value's fields are
exactly the ones its declaration lists — which is also what lets the diagnostic
offer the nearest name it does have.

## A program that provokes it

```buri fail code=no-such-field
struct Rec { export f0: Int }

fn read(r: Rec): Int {
  r.f1
}
```
