---
title: A colour's alpha is a fraction from 0 to 1
message: a colour's alpha is {alpha}, and an alpha is a fraction from 0 to 1
fix: write an alpha between 0.0 and 1.0, or reach for `Opacity` to fade the whole element
---
# A colour's alpha is a fraction from 0 to 1

```text
error: a colour's alpha is 1.5, and an alpha is a fraction from 0 to 1 [alpha-out-of-range]
```

```buri fail code=alpha-out-of-range
# from "ui/style" import { Style };

// Half again as much of the colour as there is.
let ghost: Style = .Background(.Rgba(0, 0, 0, 1.5));
```

`Rgba`'s fourth value and `Color.alpha`'s argument both mean how much of the
colour there is, so nothing outside `0.0` to `1.0` makes sense.

On a token it would fail silently: `alpha` lowers to
`color-mix(in srgb, var(--pkg-name) 50%, transparent)`, and a percentage outside
0 to 100 invalidates the whole declaration, so the element loses the colour.

Only an alpha known at compile time is checked. One built from a parameter isn't.
