# Tasks and actors

Two modules cover concurrency. `core/tasks` runs a piece of a program more than
once at a time. `core/actor` keeps state that outlives one call. Both carry
`Tasks` in their bounds, because running a program's own work concurrently is
authority like any other.

## `parallel` is the whole of `core/tasks`

```buri run
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let sizes = tasks.parallel(ctx, ["alpha", "be", "gamma"], fn(c, index, word) => {
        str.format(c, "${index}:${word.len()}")
    });
    let _ = io.println(ctx, sizes.join(ctx, " ")).ignore();
    .Ok(())
}
```

```stdout
0:5 1:2 2:5
```

The results come back **in the items' order**, whatever order the work finished
in, and each call gets the item's own index.

The `c` a task receives is the caller's whole context, so a task may do anything
its caller could and nothing it could not. It arrives as a parameter because a
lambda may not capture an effect-carrying value
([effects](../language/effects.md)).

There is no detached spawn and no handle to join: every task has finished before
`parallel` returns.

How much actually overlaps is the platform's business, and deliberately not the
signature's:

| Backend | Today |
|---|---|
| JavaScript | Started together and awaited together, so two tasks that *wait* overlap; two that compute do not, because the engine has one thread |
| Native, `--release` | Each task on a carrier of its own, so waiting and computing both overlap |
| Native, `buri run` | Sequential, in index order, on the calling carrier |

All three answer the same list, which is the point of fixing the order.

## An actor is a value

An actor is an initial state and a step, and two enums are its protocol: one for
what you may send, one for what comes back. The state is reachable only through
the messages the enum declares.

```buri name=books
from "core/actor" import { Actor, Stepped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/io" import * as io;

enum Ledger {
    Record(Int),
    Total,
}

enum Entered {
    Recorded,
    Cents(Int),
}

fn ledger<C: Alloc + Stdout + Tasks>(): Actor<C, Int, Ledger, Entered> {
    Actor {
        state: 0,
        step: fn(c, total, message) => {
            match (message) {
                .Record(cents) => Stepped { state: total + cents, answer: .Recorded },
                .Total => Stepped { state: total, answer: .Cents(total) },
            }
        },
        onStop: .Some(fn(c, total) => {
            let _ = io.println(c, "closed at ${total}").ignore();
            ()
        }),
    }
}
```

The step answers a `Stepped`: the state the next message sees, and the answer
this one gets. A message nobody needs an answer to answers a variant that says
so — `.Recorded`. `onStop` is an `Option` a literal may leave out, and leaving it
out means no hook at all.

The mailbox holds sixty-four messages and is not configurable. A send runs the
mailbox down before it answers, so the bound is what limits how much work may
wait for a driver busy somewhere else.

## `sendMessage` and `stop`

```buri run use=books
from "core/actor" import * as actor;
from "core/host" import * as host;

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let books = actor.start(ctx, ledger());
    let _ = books.sendMessage(ctx, .Record(450)).ignore();
    let _ = books.sendMessage(ctx, .Record(1905)).ignore();
    let running = match (books.sendMessage(ctx, .Total)) {
        .Ok(.Cents(n)) => n,
        .Ok(_other) => 0,
        .Err(_gone) => 0,
    };
    let _ = io.println(ctx, "running total ${running}").ignore();
    let _ = books.stop(ctx).ignore();
    let after = books.sendMessage(ctx, .Record(1));
    let _ = io.println(ctx, "after stop: ${after.isErr()}").ignore();
    .Ok(())
}
```

```stdout
running total 2355
closed at 2355
after stop: true
```

`start` gives the actor a mailbox and answers an `Address`, which is inert data.
It holds no context, so a lambda may capture one — which is what lets an address
be a request handler's shared state, or another actor's.

`stop` closes the mailbox, discards what is still in it, and runs `onStop` once
with the final state. Every `sendMessage` and second `stop` after that answers
`.Err(.Stopped)`, which is the one way an actor operation fails.

**The actor steps on the task that drives it.** `sendMessage` posts, runs the
mailbox down until its own answer is there, and hands that answer back. `stop`
closes and then runs the hook. One sender's messages arrive in order, the actor
steps each message exactly once, and a send sees the state its own message left.
So an actor is not yet a way to get work done in the background.

## Why the state goes behind a mailbox

State threaded through arguments works for as long as there is one call to
thread it through, and a long-lived program does not have one: a server's
handler answers and returns, and the next request arrives on a fresh frame.

Behind a mailbox the state is a local of the actor's own loop, and nothing else
in the program has a name for it. An update is a rebinding rather than a write,
and the protocol enum is the complete list of what anybody may do to it.
[Build a web server](./web-server.md) is that shape at work: the handler holds
an address, not a counter.

## Effects bound what a step may do

`Actor<C, S, M, R>`'s `C` is the caller's context, exactly as `parallel`'s is,
so a step may do anything the code around it could and nothing more. `ledger`
above says `C: Alloc + Stdout + Tasks` because its `onStop` prints. One whose
hook did not print would not name `Stdout`, and nothing a caller binds could add
it. The bound is settled at `actor.start`, on the context the step will be
handed ([effects and capabilities](./effects.md)).

## Testing

`step` is an ordinary function in a struct field, so a test that wants to know
what one message does calls it: no mailbox, no address, and no context but the
one the step itself needs.

```buri role=test use=books
from "core/host/testing" import { alloc, stdout, tasks };
from "core/testing/assert" import * as assert;

test "a recorded amount is added to the running total" {
    let ctx = context {
        Alloc: alloc(),
        Stdout: stdout(),
        Tasks: tasks(),
    };
    let step = ledger().step;
    assert.eq(step(ctx, 450, .Record(1905)).state, 2355);
}
```

`core/actor` ships no test double: a mailbox is a queue, and the order is the
order. `core/tasks` does have one — `tasks()` makes the order the work runs in a
value the test writes down, with `anyOrder()`, `seed(n)` and `everyOrder()`.
Both are [testing your code](./testing.md).

## Next

- [Build a web server](./web-server.md) — `Listen`, `Sockets`, and an actor
  behind a handler.
- [Effects and capabilities](./effects.md) — where a program's authority is
  written.
- [The standard library](../reference/standard-library.md) — where a message
  lives while the runtime holds it.
