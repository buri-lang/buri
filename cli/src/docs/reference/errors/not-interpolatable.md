---
title: A hole in a string holds a value nothing renders
message: '`{type}` cannot be interpolated'
note: a hole holds a primitive — `Int`, `Float`, `Bool`, `Char`, `Str` — or a value whose `Show` is derived
fix: render it first, for instance with `.show(ctx)`
---

A `Template` names no context, which is why `io.println(ctx, "hi ${name}")`
needs only `Stdout`. So a hole may hold anything the *runtime* can render from
the type's shape: a primitive, or a value whose `Show` came from a `derive`. It
may not hold a value that has to be rendered by calling a function with a
context.

```buri fail code=not-interpolatable
struct Point {
    export x: Int,
    export y: Int,
}

fn go(p: Point): Str {
    "the point is ${p}"
}
```

Add `derive Show for Point;` to `Point`'s own module and the hole above
compiles, printing what `p.show(ctx)` produces.

A **hand-written** `impl Show` is the other case, and it stays your call.
`show<C: Alloc>(self, ctx: C)` names a context the interpolation cannot reach,
so write the conversion out.

```buri ignore why="the fix, not a failure: it needs a Show impl and a ctx the page does not declare"
str.format(ctx, "the suit is ${suit.show(ctx)}")
```
