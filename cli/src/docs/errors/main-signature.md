# `main` has one shape

```text
error: `main` declares no generic parameters [main-signature]
```

## What to do

drop them: `main` is called by the runtime, so there is nothing to infer them from

## A program that provokes it

```buri fail code=main-signature
export fn main<T>(): Result<(), Str> {
  .Ok(())
}
```
