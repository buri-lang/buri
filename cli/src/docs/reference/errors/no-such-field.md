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

Correct the spelling, or name a field the type declares. Where there is a near
miss the fix names it: "if you meant `f0`, use that; if not, `Rec`'s declaration
lists its fields".

## Why

There is no structural typing and no inheritance. A value's fields are exactly
the ones its declaration lists, which is how the diagnostic can offer the
nearest name the type does have.

## A program that provokes it

```buri fail code=no-such-field
struct Rec {
    export f0: Int,
}

fn read(r: Rec): Int {
    r.f1
}
```
