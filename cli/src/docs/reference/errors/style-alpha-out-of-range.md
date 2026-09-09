---
title: A colour's alpha is a fraction from 0 to 1
message: a colour's alpha is {alpha}, and an alpha is a fraction from 0 to 1
fix: write an alpha between 0.0 and 1.0, or reach for `Opacity` to fade the whole element
---
# A colour's alpha is a fraction from 0 to 1

```text
error: a colour's alpha is 1.5, and an alpha is a fraction from 0 to 1 [style-alpha-out-of-range]
```

## What to do

Write an alpha between `0.0` and `1.0`, or reach for `Opacity` to fade the whole
element.

## Why

`Rgba`'s fourth value and `Color.alpha`'s argument are the same number and it
means the same thing: how much of the colour there is. Outside `0.0` to `1.0`
there is nothing for it to mean.

It matters most for a token, because `alpha` on one lowers to
`color-mix(in srgb, var(--pkg-name) 50%, transparent)` — and a percentage
outside 0 to 100 makes the whole declaration invalid, so the element loses the
colour rather than gaining a louder one. Refusing the number is the only way a
program hears about that.

The extractor is what refuses it, so this is about a colour the compiler could
work out. One built out of a parameter is not refused, because nothing at
compile time knows what it will be.

## A program that provokes it

```buri fail code=style-alpha-out-of-range
# from "ui/style" import { Style };

// Half again as much of the colour as there is.
let ghost: Style = .Background(.Rgba(0, 0, 0, 1.5));
```
