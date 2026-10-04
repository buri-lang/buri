---
title: `self` is written without a type
message: `self` is written without a type
note: the type is the one the `impl` head names, or the implementing type inside a `trait`
fix: delete the annotation, leaving `self`
---

```buri fail code=typed-self use=errors
impl Square {
  export fn scaled(self: Square, factor: Int): Int {
    self.side * factor
  }
}
```
