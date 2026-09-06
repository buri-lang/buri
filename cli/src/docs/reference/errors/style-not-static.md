---
title: A conditional style is known at compile time
message: {problem}
fix: write the value out, or make it a module-level `let`, or apply it outside the `On`/`At`, where it can be an inline style
---
# A conditional style is known at compile time

```text
error: a `Computed` style may not appear under `On` or `At`: a closure cannot be scoped to a pseudo-class or to a media query [style-not-static]
```

## What to do

Write the value out, or make it a module-level `let`, or apply it outside the
`On`/`At`, where it can be an inline style.

## Why

A style the compiler cannot evaluate is not normally an error. It degrades to
the inline tier, which is where `Computed` already lives. `On` and `At` are the
exception: they have nowhere to degrade *to*. There is no inline form of
`:hover` and none of `@media (min-width: 64rem)`. Both exist only as rules in a
stylesheet, and the compiler writes the stylesheet at compile time. So a style
under one of those is statically known, or the compiler rejects the program here
rather than silently losing its hover state.

## A program that provokes it

```buri fail code=style-not-static
# from "ui/style" import { Style };

// A breakpoint is a media query. A closure cannot be put inside one.
let wide: Style = .At(.Large, [.Computed(fn(scope) => [.Width(.Full)])]);
```
