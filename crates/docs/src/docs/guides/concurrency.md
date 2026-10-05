# Tasks and actors

Two modules cover concurrency. `core/tasks` runs a piece of a program more than
once at a time, and runs work in the background. `core/actor` keeps state that
outlives one call. Both carry `Tasks` in their bounds, because running a
program's own work concurrently is authority like any other.

## `parallel` runs a list of work

```buri run
from "core/io" import * as io;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let sizes = tasks.parallel(ctx, ["alpha", "be", "gamma"], fn(c, index, word) => {
        str.format(c, "${index}:${word.length()}")
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

Every task has finished before `parallel` returns, so nothing outlives the
context that granted it.

How much actually overlaps is the platform's business, and deliberately not the
signature's:

| Backend | Today |
|---|---|
| JavaScript | Started together and awaited together, so two tasks that *wait* overlap; two that compute do not, because the engine has one thread |
| Native, `--release` | Each task on a thread of its own, so waiting and computing both overlap |
| Native, `buri run` | Sequential, in index order, on the calling thread |

All three answer the same list, which is the point of fixing the order.

## `scope` and `spawn` run work in the background

A socket that stays open, a retry, a timer. That work is not a list, and
`parallel` is the wrong shape for it: it starts once and runs beside everything
else.

`scope` opens a place for it, `spawn` puts one task there, and the scope returns
once the body and every task spawned into it have finished.

```buri run
from "core/io" import * as io;
from "core/tasks" import * as tasks;
from "core/time" import * as time;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Clock, Stdout, Tasks };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let _ = tasks.scope(ctx, fn(c, here) => {
        tasks.spawn(c, here, fn(c2) => {
            let _ = time.sleep(c2, time.milliseconds(5));
            let _ = io.println(c2, "the timer fired").ignore();
            ()
        })
    });
    let _ = io.println(ctx, "the scope is closed").ignore();
    .Ok(())
}
```

```stdout
the timer fired
the scope is closed
```

A task that sleeps is a timer, as long as you hold a scope. Code that holds
only a context — a callback, a handler — schedules with `after` instead, below.

**`Scope` is inert.** It holds no context, so a lambda may capture one, and an
interface can hand a scope to a handler that spawns into it later. That is what
makes a page work: `mount` returns, `main` returns, and a click still opens a
socket in the scope the page was built in.

**A library cannot spawn.** It exposes a `run` and the application spawns it,
because a scope is the application's to open.

**Stopping is cooperative.** A spawned task can't be cancelled. It ends when its own body
ends, so a loop stops by finding its socket closed, or by asking an actor
whether to carry on. An abort is a write to standard error and an exit
([effects](../language/effects.md)), never something a second task survives.

In a native `--release` build a spawned task starts at once, on a thread of its
own, while the body keeps running. Everywhere else — `buri run`, JavaScript, and
tests on every backend — the body runs first, then the scope runs what was
spawned in rounds, one round for whatever the last one spawned, until nothing is
waiting. So spawned tasks overlap on JavaScript and run one after another under
`buri run`. A task spawned *after* the body has returned — which on a page is
what a handler does — runs on the task that spawned it.

So under `buri run` a task that never ends starves the ones behind it. Spawn a
socket loop and then a timer, and the timer never starts. Build with
`--release`, or run on JavaScript, and both run.

## A timer needs no scope

`after` runs a function once a `Duration` has passed and answers at once.
`cancel` stops it. Both need only `Tasks`, so they work on every platform and
from any code that holds a context.

```buri run
from "core/io" import * as io;
from "core/tasks" import * as tasks;
from "core/time" import * as time;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout, Tasks };

fn tick<C: Stdout + Tasks>(ctx: C, n: Int): () {
    let _ = io.println(ctx, "tick ${n}").ignore();
    if (n < 3) {
        let _ = tasks.after(ctx, time.milliseconds(10), fn(c) => tick(c, n + 1));
        ()
    } else {
        ()
    }
}

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let never = tasks.after(ctx, time.milliseconds(5), fn(c) => {
        io.println(c, "never").ignore()
    });
    let _ = tasks.cancel(ctx, never);
    let _ = tasks.after(ctx, time.milliseconds(5), fn(c) => tick(c, 1));
    let _ = io.println(ctx, "main returned").ignore();
    .Ok(())
}
```

```stdout
main returned
tick 1
tick 2
tick 3
```

`run` gets a context of its own when it fires: the one `after` was called with.
A periodic tick is a `run` that schedules the next one.

**A pending timer keeps the program running** after `main` returns `.Ok`, until
it fires or is cancelled — node's event loop rule, on every platform. A `main`
that answers `.Err`, an abort and `exitWith` end the program at once.

**A timer fires while the program waits.** On JavaScript it's a `setTimeout`.
A native program fires it on `main`'s thread whenever that thread waits — a
sleep, a fetch, a `parallel` being joined — and after `main` returns. It never
interrupts code that is computing. Under `buri run` a server's workers wait on
`main`'s thread for a connection, so a timer started there fires once the
server stops. Build with `--release`, where the workers have threads of their
own, and it fires on time.

## An actor is a value

An actor is an initial state and a step, and two enums are its protocol: one for
what you may send, one for what comes back. The state is reachable only through
the messages the enum declares.

```buri name=books
from "core/actor" import { Actor, Stepped };
from "core/io" import * as io;
from "platform/effect" import { Allocator, Stdout, Tasks };

enum Ledger {
    Record(Int),
    Total,
}

enum Entered {
    Recorded,
    Cents(Int),
}

fn ledger<C: Allocator + Stdout + Tasks>(): Actor<C, Int, Ledger, Entered> {
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

The one way a program reaches the bound is a step posting to its own actor,
because nothing drains while a step holds the state. Keep a step's fan-out at
sixty-four or under: a sixty-fifth message from there waits for room nobody is
coming to make.

## `sendMessage` and `stop`

```buri run use=books
from "core/actor" import * as actor;
from "native" import { NativeHost };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
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
`.Err(.Stopped)`.

`sendMessage` fails with a `SendError`, which says why:

```buri
from "core/actor" import { SendError };

export fn shown(answer: Result<Int, SendError>): Str {
    match (answer) {
        .Ok(_count) => "answered",
        .Err(.Stopped) => "the actor was stopped",
        .Err(.TimedOut) => "waited thirty seconds and gave up",
        .Err(.WouldDeadlock) => "sent from the actor's own step",
    }
}
```

**The actor steps on the task that drives it.** `sendMessage` posts, runs the
mailbox down until its own answer is there, and hands that answer back. `stop`
closes and then runs the hook. One sender's messages arrive in order, the actor
steps each message exactly once, and a send sees the state its own message left.
So an actor is not yet a way to get work done in the background.

Two tasks may send at once. A sender that finds another task stepping the actor
waits for that step to finish, then gets its own answer, or `.Err(.TimedOut)`
after thirty seconds.

A step that sends to the actor running it gets `.Err(.WouldDeadlock)` back, and
so does a task that step started. The state is already out, and waiting for it
would be waiting for itself, so the send doesn't wait. The message is posted all
the same, and the loop already running steps it before it puts the state back.

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
above says `C: Allocator + Stdout + Tasks` because its `onStop` prints. One whose
hook did not print would not name `Stdout`, and nothing a caller binds could add
it. The bound is settled at `actor.start`, on the context the step will be
handed ([effects and capabilities](./effects.md)).

## Testing

`step` is an ordinary function in a struct field, so a test that wants to know
what one message does calls it: no mailbox, no address, and no context but the
one the step itself needs.

```buri role=test use=books
from "core/testing/assert" import * as assert;
from "platform/effect/testing" import { alloc, stdout, tasks };

test "a recorded amount is added to the running total" {
    let ctx = context {
        Allocator: alloc(),
        Stdout: stdout(),
        Tasks: tasks(),
    };
    let step = ledger().step;
    assert.equal(step(ctx, 450, .Record(1905)).state, 2355);
}
```

`core/actor` ships no test double: a mailbox is a queue, and the order is the
order. `core/tasks` does have one — `tasks()` makes the order the work runs in a
value the test writes down, with `anyOrder()`, `seed(n)` and `everyOrder()`, and
it covers a spawned task too, because a scope runs its round through
`Tasks.parallel`. So a test of background work asserts an order it chose rather
than one it hoped for, and none of it waits on real time. Both are
[testing your code](./testing.md).

## Next

- [Build a web server](./web-server.md) — `Listen`, `Sockets`, and an actor
  behind a handler.
- [Effects and capabilities](./effects.md) — where a program's authority is
  written.
- [The standard library](../reference/standard-library.md) — where a message
  lives while the runtime holds it.
