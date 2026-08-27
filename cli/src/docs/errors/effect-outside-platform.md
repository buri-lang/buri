---
title: Only a platform module declares an effect
message: only a platform module may declare an effect
note: the set of things a Buri program can do to the world is fixed by its platform rather than open-ended
fix: declare it as a plain `trait`, or move it into a platform module
---
# Only a platform module declares an effect

```text
error: only a platform module may declare an effect [effect-outside-platform]
```

## What to do

Declare it as a plain `trait`, or move it into a platform module.

## Why

The set of things a program can do to the world is fixed by its platform rather
than open-ended. An `effect` a library could declare would be authority a
library could mint.

## A program that provokes it

```buri fail code=effect-outside-platform
effect Mischief {
  fn meddle(self): ();
}
```
