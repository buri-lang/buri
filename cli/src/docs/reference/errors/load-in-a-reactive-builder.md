---
title: A reactive builder is synchronous, so it may not `load`
message: a reactive builder may not `load` — `ui.{builder}`'s body must be synchronous
fix: move the `load` onto the press or the route that reaches this subtree, and hand the builder what it loaded
---
# A reactive builder is synchronous, so it may not `load`

```text
error: a reactive builder may not `load` — `ui.computed`'s body must be synchronous [load-in-a-reactive-builder]
```

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

`ui.computed`, `ui.each` and `ui.rebuild` call their closure to build a
subtree. `load` waits for a chunk, which makes the closure `async`: it returns a
promise, not a node. The page comes up blank, and the first failed render leaves
the rest of it inert.

A handler, `fn(C, Event) => ()`, may wait: it runs to completion and writes
signals when the wait is over. So `load` goes on the press or route that decides
the subtree is wanted, and the builder reads what it loaded from a signal. On a
website, that's the route match.
