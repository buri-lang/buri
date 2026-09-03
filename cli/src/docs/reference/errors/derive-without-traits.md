---
title: A `derive` clause names at least one trait
message: a `derive` clause names no traits
note: `derive` generates one implementation per trait it names, so a clause naming none would generate nothing; delete it, or name what the type should derive
fix: name the traits between `derive` and `for`, as in `derive Eq, Show for Meters;`
---
# A `derive` clause names at least one trait

```text
error: a `derive` clause names no traits [derive-without-traits]
```

## What to do

Name the traits between `derive` and `for`, as in `derive Eq, Show for Meters;`.

## Why

`derive` generates one implementation per trait it names, so a clause naming
none would generate nothing at all. Delete it, or say what the type should
derive.

## A program that provokes it

```buri fail code=derive-without-traits
struct Meters(export Float);

derive for Meters;

export fn main(): Result<(), Str> {
  .Ok(())
}
```
