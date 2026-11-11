---
title: A labelled stack has a role
message: a `stack` with a `label` takes a `role`, because a name has to name something a reader is told about
fix: give the stack a `role`, or drop the `label`
---
# A labelled stack has a role

```text
error: a `stack` with a `label` takes a `role`, because a name has to name something a reader is told about [label-without-role]
```

```buri fail code=label-without-role
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

// A plain box isn't announced, so its name never is either.
fn chart<C>(bars: [Node<C>]): Node<C> {
    ui.stack({
        styles: [.Layout(.Layers)],
        children: bars,
        label: .Some(.Const("Value over the past year, from 12 to 40")),
    })
}
```

A `label` is the name a reader hears for what a stack is. A stack with no
`role` is a plain box a reader isn't told about, so there's nothing to name.
Give it the role it plays:

```buri
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

fn chart<C>(bars: [Node<C>]): Node<C> {
    ui.stack({
        styles: [.Layout(.Layers)],
        children: bars,
        role: .Some(.Group),
        label: .Some(.Const("Value over the past year, from 12 to 40")),
    })
}
```

A decorative stack can't take a label either: it's hidden from a reader, and it
takes no role ([`decorative-with-role`](decorative-with-role.md)).

The compiler reads both fields where the stack is written, in the call or not.
A pair it can't read is refused too, because it can't tell that the stack has
a role.
