---
title: A hole in a string holds a primitive
message: '`{type}` cannot be interpolated'
note: a hole holds an `Int`, a `Float`, a `Bool`, a `Char`, or a `Str`
fix: render it first, for instance with `.show(ctx)`
---

```buri fail code=not-interpolatable
struct Point { export x: Int, export y: Int }

fn go(p: Point): Str {
  "the point is ${p}"
}
```
