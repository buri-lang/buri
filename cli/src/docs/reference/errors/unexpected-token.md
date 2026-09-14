---
title: The grammar expected something else here
message: expected {expected}, found {found}
---
# The grammar expected something else here

```text
error: expected a declaration, found `@` [unexpected-token]
```

## A program that provokes it

```buri fail code=unexpected-token
fn one(): Int {
  1
}

@
```
