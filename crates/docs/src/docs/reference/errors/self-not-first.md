---
title: `self` is the first parameter or nothing
message: `self` may appear only as {position}
fix: move it to the front, or rename it if this parameter is not the receiver
---
# `self` is the first parameter or nothing

```text
error: `self` may appear only as a function's first parameter [self-not-first]
```

```buri fail code=self-not-first use=errors
fn scaled(factor: Int, self): Int {
  self.side * factor
}
```

`self` first and `ctx` right after is the whole calling convention, so the front
of a signature tells you what a function takes and what it may do.
