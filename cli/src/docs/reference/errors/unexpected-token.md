---
title: The grammar expected something else here
message: expected {expected}, found {found}
---
# The grammar expected something else here

```text
error: expected a declaration, found `@` [unexpected-token]
```

## What to do

Write what the message names. This is the parser's catch-all, so the useful half
of it is always the "expected" part. A mistake the parser can name — a
separator, a terminator, an arrow, a delimiter that never closed — carries its
own code instead.

## A program that provokes it

```buri fail code=unexpected-token
fn one(): Int {
  1
}

@
```
