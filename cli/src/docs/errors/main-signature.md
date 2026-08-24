# `main` has one shape

```text
error: `main` declares no generic parameters [main-signature]
```

## What to do

Drop them. `main` takes no parameters, declares no generic parameters, and
returns `Result<(), Str>`.

## Why

`main` is called by the runtime rather than by a program, so there is no call
site to infer a type argument from and nothing to pass an argument in.

## A program that provokes it

```buri fail code=main-signature
export fn main<T>(): Result<(), Str> {
  .Ok(())
}
```
