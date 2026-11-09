---
title: A function the page asks for an answer doesn't wait
message: this function waits, and the page needs its answer at once
label: waits
note: a page builds a `ui.each` or `ui.rebuild` row, and asks an `onKey` whether it claimed the key, without waiting for the answer
fix: do the waiting in a press handler, or in `main` before `ui.mount`, and keep what it got in a signal the row reads
reproduction: none
---
# A function the page asks for an answer doesn't wait

```text
error: this function waits, and the page needs its answer at once [answer-waits]
```

A row and an `onKey` answer the page while it's busy: a row while it draws the
list, and an `onKey` while the browser is still handling the key. Neither can
sleep, read storage, call the network or reach anything else that waits.

A press handler can wait, because the page doesn't need an answer from it.
Wait there and put the result in a signal the rows read:

```buri
# from "core/time" import * as time;
# from "platform/effect" import { Clock, Ui };
# from "ui/node" import * as ui;
# from "ui/node" import { Node };
# from "ui/signal" import { Signal };

fn names<C: Clock + Ui>(loaded: Signal<[Str]>): Node<C> {
    ui.stack({
        styles: [],
        children: [
            ui.button({
                label: .Const("Load"),
                styles: [],
                onPress: .Some(fn(c) => {
                    let _ = time.sleep(c, time.milliseconds(10));
                    loaded.set(c, ["Ada", "Grace"])
                }),
            }),
            ui.each(.Cell(loaded), fn(n) => n, fn(_c, n, _i) => {
                ui.text({ content: .Const(n) })
            }),
        ],
    })
}
```
