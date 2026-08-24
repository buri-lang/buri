# A conditional style is known at compile time

```text
error: a `Computed` style may not appear under `On` or `At`: a closure cannot be scoped to a pseudo-class or to a media query [style-not-static]
```

## What to do

write the value out, or make it a `const`, or apply it outside the `On`/`At`, where it can be an inline style

## Why

`ui/style` has two tiers. Everything except `Computed` is *static*: the compiler
evaluates it, turns each distinct property value into one atomic class, and
writes the classes into the stylesheet that ships with the artifact. `Computed`
is the other tier — a value driven by a signal, applied inline to one element
and re-serialised whenever it changes.

A style the compiler cannot evaluate is not normally an error: it quietly
degrades to the inline tier, which is where `Computed` already lives.

`On` and `At` are the exception, because they have nowhere to degrade *to*.
There is no inline form of `:hover`, and there is no inline form of
`@media (min-width: 64rem)`; both exist only as rules in a stylesheet, and the
stylesheet is written at compile time because nothing in a Buri user interface
is generated at run time. So a style under one of those is statically known, or
the program is rejected here rather than silently losing its hover state.

## A program that provokes it

```buri fail code=style-not-static
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "ui/style" import { Style };

// A breakpoint is a media query. A closure cannot be put inside one.
const wide: Style = .At(.Large, [.Computed(fn(scope) => [.Width(.Full)])]);

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = wide;
  io.println(ctx, "never gets here")
}
```

Compiled by the test suite, which checks that it still produces
`style-not-static` — so this page cannot describe an error the compiler has
stopped emitting.
