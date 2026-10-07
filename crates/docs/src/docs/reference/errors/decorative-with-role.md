---
title: A decorative stack has no role
message: a `stack` that is decorative takes no `role`, because a hidden element has nothing to announce
fix: drop the `role`, or the `isDecorative`
---
# A decorative stack has no role

```text
error: a `stack` that is decorative takes no `role`, because a hidden element has nothing to announce [decorative-with-role]
```

```buri fail code=decorative-with-role
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

// A list nobody is told about is not a list.
fn crumbs<C>(): Node<C> {
    ui.stack({
        styles: [],
        children: [ui.text({ content: .Const("/") })],
        role: .Some(.List),
        isDecorative: .Some(true),
    })
}
```

`isDecorative` hides a stack and everything in it from assistive technology. A
`role` is what assistive technology is told, so the two can't both hold.

Wrap the decorative part in a stack of its own inside the one with the role:

```buri
# from "ui/node" import * as ui;
# from "ui/node" import { Node };

fn crumb<C>(name: Str): Node<C> {
    ui.stack({
        styles: [.Layout(.Row)],
        children: [
            ui.text({ content: .Const(name) }),
            ui.stack({
                styles: [],
                children: [ui.text({ content: .Const("/") })],
                isDecorative: .Some(true),
            }),
        ],
        role: .Some(.ListItem),
    })
}
```

The compiler reads both fields at the call site. A pair it can't read is
refused too, because it can't tell that they don't hold together.
