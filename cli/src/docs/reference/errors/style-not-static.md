---
title: A conditional style is known at compile time
message: {problem}
fix: write the value out, or make it a module-level `let`, or apply it outside the `On`/`At`, where it can be an inline style
---
# A conditional style is known at compile time

```text
error: a `Computed` style may not appear under `On` or `At`: a closure cannot be scoped to a pseudo-class or to a media query [style-not-static]
```

```buri fail code=style-not-static
# from "ui/style" import { Style };

// A breakpoint is a media query. A closure cannot be put inside one.
let wide: Style = .At(.Large, [.Computed(fn(scope) => [.Width(.Full)])]);
```

A style the compiler can't evaluate usually falls back to an inline style, where
`Computed` already lives. `On` and `At` can't: `:hover` and `@media` have no
inline form. They exist only as stylesheet rules, which the compiler writes at
compile time.
