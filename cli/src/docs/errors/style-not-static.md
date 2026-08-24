# A conditional style is known at compile time

```text
error: a `Computed` style may not appear under `On` or `At`: a closure cannot be scoped to a pseudo-class or to a media query [style-not-static]
```

## What to do

Write the value out, or make it a `const`, or apply it outside the `On`/`At`,
where it can be an inline style.

## Why

A style the compiler cannot evaluate is not normally an error — it degrades to
the inline tier, which is where `Computed` already lives. `On` and `At` are the
exception because they have nowhere to degrade *to*: there is no inline form of
`:hover` and none of `@media (min-width: 64rem)`. Both exist only as rules in a
stylesheet, and the stylesheet is written at compile time. So a style under one
of those is statically known, or the program is rejected here rather than
silently losing its hover state.

## A program that provokes it

```buri fail code=style-not-static
# from "ui/style" import { Style };
// A breakpoint is a media query. A closure cannot be put inside one.
const wide: Style = .At(.Large, [.Computed(fn(scope) => [.Width(.Full)])]);
```
