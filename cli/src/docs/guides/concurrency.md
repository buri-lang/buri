# Tasks and actors

Two modules cover concurrency, and they answer two different questions.
`core/tasks` runs a piece of a program more than once at a time. `core/actor`
keeps state that outlives one call. Both carry `Tasks` in their bounds, because
running a program's own work concurrently is authority: a signature that names
it says the caller was granted that right, and a reader looking for what a
program can do finds it where they look for everything else.

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
in. That promise is the reason this module has one signature rather than a
family of them: a result list in completion order would be untestable, because
there is no way to write "assert one of these six outputs". Each call is handed
the item's own index, so a task can name where it is without a counter to keep
it in.

The `c` a task receives is the caller's whole context — every effect `ctx`
carried — so a task may do anything its caller could and nothing it could not.
It arrives as a parameter rather than by capture because a lambda may not
capture an effect-carrying value ([effects](../language/effects.md)).

There is no detached spawn and no handle to join: every task has finished before
`parallel` returns. That is what keeps "a program that never names `host.fs`
cannot read a file" true of the program's lifetime and not only of its call
graph.

How much of it actually overlaps is the platform's business and deliberately not
the signature's:

| Backend | Today |
|---|---|
| JavaScript | Started together and awaited together, so two tasks that *wait* overlap; two that compute do not, because the engine has one thread |
| Native, `--release` | Each task on a carrier of its own, so waiting and computing both overlap |
| Native, `buri run` | Sequential, in index order, on the calling carrier |

All three answer the same list, which is the point of fixing the order.

## An actor is a value

An actor is an initial state and a step, and the enum it steps over is its
protocol. Nothing else about it is addressable: the state is reachable only
through the messages the enum declares.

```buri name=books
from "core/actor" import { Actor, Reply };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/io" import * as io;

enum Ledger {
    Record(Int),
    Total(Reply<Int>),
}

fn ledger<C: Alloc + Stdout + Tasks>(): Actor<C, Int, Ledger> {
    Actor {
        state: 0,
        step: fn(c, total, message) => {
            match (message) {
                .Record(cents) => total + cents,
                .Total(reply) => {
                    let _ = reply.answer(c, total).ignore();
                    total
                },
            }
        },
        onStop: .Some(fn(c, total) => {
            let _ = io.println(c, "closed at ${total}").ignore();
            ()
        }),
        mailbox: .Some(8),
    }
}
```

A variant carrying no `Reply` is a `send`; one carrying a `Reply<R>` is an `ask`
that yields an `R`. The pairing between a request and its answer is therefore
declared exactly once, in the enum. `onStop` and `mailbox` are `Option` fields a
literal may leave out: no hook at all, and `core/actor`'s own `MAILBOX` of 64.

## `send`, `ask`, and `stop`

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
    let _ = books.send(ctx, .Record(450)).ignore();
    let _ = books.send(ctx, .Record(1905)).ignore();
    let running = books.ask(ctx, fn(reply) => .Total(reply)).withDefault(0);
    let _ = io.println(ctx, "running total ${running}").ignore();
    let _ = books.stop(ctx).ignore();
    let after = books.send(ctx, .Record(1));
    let _ = io.println(ctx, "after stop: ${after.isErr()}").ignore();
    .Ok(())
}
```

```stdout
running total 2355
closed at 2355
after stop: true
```

`start` gives the actor a mailbox and answers an `Address`, which is inert data:
it holds no context, so a lambda may capture one — which is what lets an address
be a request handler's shared state, or another actor's.

`ask` takes the *constructor* — `fn(reply) => .Total(reply)` rather than
`.Total`, because a bare variant name is checked against the type it is used at,
and there it is used at a function type. `stop` closes the mailbox, discards
what is still in it, and runs `onStop` once with the final state; every `send`,
`ask` and second `stop` after that answers `.Err(.Stopped)`, which is the one
way an actor operation fails.

**The actor steps on the task that drives it.** `send` posts and returns; `ask`
posts and then runs the mailbox down until its reply is there; `stop` closes and
then runs the hook. That is a scheduling decision rather than a semantic one —
one sender's messages arrive in order, a message is stepped exactly once, and
`ask` sees the state its own message left — but it means an actor is not yet a
way to get work done in the background.

## Why the state goes behind a mailbox

State threaded through arguments works for as long as there is one call to
thread it through. A long-lived program does not have one: a server's handler
answers and returns, and the next request arrives on a fresh frame and possibly
a different worker, so there is nowhere for a running total to sit. Threading it
would mean handing every function the whole of the program's state and trusting
callers to pass on what they were given.

A mailbox removes the question. The state is a local of the actor's own loop and
nothing else in the program has a name for it, so an update is a rebinding
rather than a write, and the protocol enum is the complete list of what anybody
may do to it. [Build a web server](./web-server.md) is that shape at work: the
handler holds an address, not a counter.

## Effects bound what a step may do

`Actor<C, S, M>`'s `C` is the caller's context, exactly as `parallel`'s is. A
step may therefore do anything the code around it could — allocate, print, read
a clock, ask another actor — and nothing more. `ledger` above says
`C: Alloc + Stdout + Tasks` because its `onStop` prints; one whose hook did not
would not name `Stdout`, and nothing a caller binds could add it.

That is [effects and capabilities](./effects.md) with no exception carved out
for concurrency: the bound is the whole claim, and it is settled at
`actor.start`, on the context the step will be handed.

## Testing

`step` is an ordinary function in an ordinary struct field, so a test that wants
to know what one message does calls it — no mailbox, no address, and no context
but the one the step itself needs:

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
    assert.eq(step(ctx, 450, .Record(1905)), 2355);
}
```

`core/actor` ships no test double, and that is a property of the shape rather
than an omission: a mailbox is a queue, the order is the order, and the one
thing a double would decide is decided in Buri where a test can read it.
`core/tasks` does have one — `tasks()` makes the order the work runs in a value
the test writes down, with `anyOrder()`, `seed(n)` and `everyOrder()`. Both are
[testing your code](./testing.md).

## Next

- [Build a web server](./web-server.md) — `Listen`, `Sockets`, and an actor
  behind a handler.
- [Effects and capabilities](./effects.md) — where a program's authority is
  written.
- [The standard library](../reference/standard-library.md) — the map, including
  where a message lives while the runtime holds it.
