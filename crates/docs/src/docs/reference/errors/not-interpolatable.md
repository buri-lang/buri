---
title: A string hole holds a renderable value
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

```buri
# from "core/str" import * as str;
# from "platform/effect" import { Allocator };
#
# enum Suit {
#     Hearts,
#     Spades,
# }
#
# impl Show for Suit {
#     fn show<C: Allocator>(self, ctx: C): Str {
#         match (self) {
#             .Hearts => "hearts",
#             .Spades => "spades",
#         }
#     }
# }
#
fn describe<C: Allocator>(ctx: C, suit: Suit): Str {
    str.format(ctx, "the suit is ${suit.show(ctx)}")
}
```
