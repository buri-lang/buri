# A `derive` clause names at least one trait

```text
error: a `derive` clause names no traits [derive-without-traits]
```

## What to do

name the traits between `derive` and `for`, as in `derive Eq, Show for Meters;`

## Why

`derive` generates one implementation per trait it names, so a clause naming none would generate nothing; delete it, or name what the type should derive

## A program that provokes it

```buri fail code=derive-without-traits
struct Meters(export Float);

derive for Meters;

export fn main(): Result<(), Str> {
  .Ok(())
}
```

Compiled by the test suite, which checks that it still produces `derive-without-traits` — so
this page cannot describe an error the compiler has stopped emitting.
