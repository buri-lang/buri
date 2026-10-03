---
title: A hole in a string holds a value nothing renders
message: '`{type}` cannot be interpolated'
note: a hole holds a primitive — `Int`, `Float`, `Bool`, `Char`, `Str` — or a value whose `Show` is derived
fix: render it first, for instance with `.show(ctx)`
---

```buri fail code=not-interpolatable
struct Point {
    export x: Int,
    export y: Int,
}

fn go(p: Point): Str {
    "the point is ${p}"
}
```

Add `derive Show for Point;` to `Point`'s own module and this compiles, printing
what `p.show(ctx)` produces.

A `Template` names no context, so a hole holds only what the runtime can render
from the type's shape. A hand-written `impl Show` needs a context the hole can't
reach, so call it yourself:

```buri ignore why="the fix, not a failure: it needs a Show impl and a ctx the page does not declare"
str.format(ctx, "the suit is ${suit.show(ctx)}")
```
