---
title: A `test` lives in a test source
message: a `test` declaration is legal only in a test source
note: a module is a test source because a rule lists it in `test.sources`; that is the only thing that makes one
fix: move it into a file listed in the target's `test.sources`
---
# A `test` lives in a test source

```text
error: a `test` declaration is legal only in a test source [test-outside-test-source]
```

## What to do

Move it into a file listed in the target's `test.sources`.

## Why

A module is a test source because a rule lists it there. Nothing else makes one.
So a `test` in production code is not a test the runner has missed. It is a
declaration in a file the runner will never open.

## A program that provokes it

```buri fail code=test-outside-test-source
test "a test in a binary source" {
    let n = 1;
}
```
