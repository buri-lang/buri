---
title: A type with no finite value cannot be constructed
message: `{name}` can never be constructed
note: every variant recurses, so building one would need one already
fix: give `{name}` a variant that does not mention itself, the way `.None` terminates an `Option`
---
# A type with no finite value cannot be constructed

```text
error: `Endless` can never be constructed [uninhabited]
```

## What to do

Give the type a variant that does not mention itself, the way `.None`
terminates an `Option`.

## Why

Every variant recurses, so building one would need one already. There is no
laziness and no null to break the cycle with, which is why this is caught at
the declaration rather than at the first attempt to construct one.

## A program that provokes it

```buri fail code=uninhabited
enum Endless {
    Node(Endless),
}
```
