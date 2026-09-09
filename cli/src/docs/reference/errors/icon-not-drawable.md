---
title: An icon's artwork is written out, and holds only shapes
message: {problem}
fix: write the artwork out at the call site, and keep it to an `<svg>` and the shapes inside it
---
# An icon's artwork is written out, and holds only shapes

```text
error: an icon may hold only an `<svg>` and the shapes inside it, and this one holds a `<script>` [icon-not-drawable]
```

## What to do

Write the artwork out at the call site, and keep it to an `<svg>` and the
shapes inside it.

## Why

`ui/node`'s `icon` puts the artwork *in* the tree: the source is written into
the document as an `<svg>`, which is what lets `currentColor` inside it be the
element's own `Foreground`. That is the whole point of the constructor — an
icon follows the text beside it, and a theme switch recolours every icon for
nothing — and it is also why the compiler reads the source rather than shipping
it unseen. Whatever the artwork says lands in the document.

So an icon holds an `<svg>` and the shapes under it — `g`, `path`, `rect`,
`circle`, `ellipse`, `line`, `polyline`, `polygon` — carrying only the
attributes that draw one. A script, an event handler, a `<use>` or an `<image>`
pointing somewhere else is refused here rather than quietly dropped by the
renderer, which builds nothing but the list above.

The artwork also has to be a string the compiler can read, so it is written at
the call site or in a module-level `let`. One that is not there until the
program runs cannot be checked before it is put in a page.

## A program that provokes it

```buri fail code=icon-not-drawable
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

// A picture that points somewhere else is not artwork this can draw.
fn logo<C>(): Node<C> {
    ui.icon([], "<svg viewBox='0 0 24 24'><image href='/logo.png'/></svg>")
}
```
