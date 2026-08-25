---
title: A `.Variant` needs a known expected type
message: `.{variant}` needs a known expected type
fix: write the qualified form, as in `Option.{variant}(...)`, or annotate what this value is being used as
---
# A `.Variant` needs a known expected type

```text
error: `.Some` needs a known expected type [unannotated-variant]
```

## What to do

Write the qualified form — `Option.Some(...)` — or annotate what this value is
being used as.

## Why

`.Some` is shorthand for "the `Some` of whatever type is expected here", and a
`let` with no annotation expects nothing. Inference flows into the shorthand
rather than out of it, which is what keeps two enums free to share a variant
name.

## A program that provokes it

```buri fail code=unannotated-variant
fn mystery(): Int {
  let v = .Some(3);
  match (v) {
    .Some(n) => n,
    .None => 0,
  }
}
```
