---
title: A reactive builder is synchronous, so it may not `load`
message: a reactive builder may not `load` — `ui.{builder}`'s body must be synchronous
fix: move the `load` onto the press or the route that reaches this subtree, and hand the builder what it loaded
---
# A reactive builder is synchronous, so it may not `load`

```text
error: a reactive builder may not `load` — `ui.computed`'s body must be synchronous [load-in-a-reactive-builder]
```

## What to do

Move the `load` onto the press or the route that reaches this subtree, and hand
the builder the value it loaded. A handler waits, so `load` belongs there; a
reactive builder does not, so it must be synchronous.

## Why

`ui.computed`, `ui.each` and `ui.rebuild` hand the renderer a closure that it
calls to make a subtree. `core/lazy`'s `load` waits for a chunk to arrive, so a
builder that reaches it is an `async` function — it answers a promise, not a
node. The renderer has no node to render, so the page comes up blank and the
first failed render leaves the rest of it inert.

A handler is the opposite: `fn(C, Event) => ()` may wait, because it runs to
completion and writes signals when the wait is over. So the split a chunk needs
is already there — `load` on the press or the route that decides a subtree is
wanted, and a plain, synchronous builder that reads what the wait produced. On a
website that route match is where the `load` goes, and the builder reads the
loaded page out of a signal the match wrote.

## A program that provokes it

```buri fail code=load-in-a-reactive-builder
# from "core/lazy" import * as lazy;
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

fn page<C>(): Node<C> {
    ui.text({ content: .Const("page") })
}

// `computed` builds its subtree synchronously, so a `load` in it makes the
// builder answer a promise the renderer cannot render.
fn body<C>(): Node<C> {
    ui.computed(fn(_scope) => {
        let build = lazy.load(page);
        build()
    })
}
```
