---
title: An icon's artwork is written out, and holds only shapes
message: {problem}
fix: write the artwork out at the call site, and keep it to an `<svg>` and the shapes inside it
---
# An icon's artwork is written out, and holds only shapes

```text
error: an icon may hold only an `<svg>` and the shapes inside it, and this one holds a `<script>` [icon-not-drawable]
```

```buri fail code=icon-not-drawable
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

// A picture that points somewhere else is not artwork this can draw.
fn logo<C>(): Node<C> {
    ui.image({
        source: .Const("<svg viewBox='0 0 24 24'><image href='/logo.png'/></svg>"),
        alt: .Decorative,
        styles: [],
    })
}
```

A decorative `image`, one whose `alt` is `.Decorative`, writes its source into
the document as an `<svg>`. That's what lets `currentColor` inside it follow the
element's `Foreground`, so a theme switch recolours it for free. It's also why
the compiler reads the source: whatever the artwork says lands in the page.

The allowed shapes are `g`, `path`, `rect`, `circle`, `ellipse`, `line`,
`polyline` and `polygon`, with only the attributes that draw them. A script, an
event handler, a `<use>`, or an `<image>` pointing elsewhere is refused rather
than silently dropped by the renderer.

The artwork must be a string the compiler can read: written at the call site or
in a module-level `let`, not computed at run time.
