# `Self` names the implementing type

```text
error: `Self` is legal only inside a `trait` or `impl` [self-type-outside-impl]
```

## What to do

Name the type itself here.

## Why

`Self` stands for the implementing type, and outside a `trait` or an `impl`
there is none for it to stand for.

## A program that provokes it

```buri fail code=self-type-outside-impl
fn identity(x: Int): Self { x }
```
