# Test targets and the testing host

Tests live inside the target they test. You declare them in its build rule, and
they reach only what a dependent could reach. There is no separate test target
and no way to test a private function directly.

This page is the exact rules: the `test` block, what a test source may import,
every member of `core/host/testing`, and what a run does with a suite. To learn
to write one from scratch, read [testing your code](../../guides/testing.md).

For the language side, read [`language/programs.md` §11.2 and
§11.3](../../language/programs.md). It covers the `test` declaration,
`core/testing/assert`, and the `context` form.

## The `test` block

```textproto schema=build
library {
    sources: ["cents.buri", "parse.buri"]

    test {
        sources: ["test/cents.buri", "test/parse.buri"]
    }
}
```

A module listed in `test.sources` is a **test source**. Nothing else makes one.
Only a test source may hold `test` declarations and import test-only modules,
and the compiler puts it in a test binary rather than in the library.

- A `test` declaration takes no parameters and returns nothing. It passes unless
  an assertion in it fails, and a failing assertion ends that test and no other.
- **Use a title once per file.** Two tests in one module that share one are a
  compile error (`duplicate-test-name`). The title is how the runner reports a
  failure and how `--filter` picks a test out. Two files of one suite may share
  a title: each failure names its own file and its own line.
- A test source and `main`'s body are the only places in the language that
  **create** a context rather than receive one. That is why only a test source
  may import `core/host/testing`.

A test source is code, so [`buri lint`](../cli/lint.md#what-it-reads) holds it
to every rule it holds a library source to, and `--fix` rewrites it the same
way.

`core/testing/assert` is an ordinary module. The name `assert` comes from
`import * as assert`, and nothing stops a file calling it something else.

| Function | Answers |
|---|---|
| `assert.eq`, `assert.notEq`, `assert.isTrue`, `assert.isFalse`, `assert.contains`, `assert.isEmpty`, `assert.notEmpty`, `assert.len`, `assert.gt`, `assert.ge`, `assert.lt`, `assert.le`, `assert.approxEq` | `()`, so the call stands alone as a statement |
| `assert.ok`, `assert.err`, `assert.some` | The unwrapped value |

The statement rule asks for the type, not the shape. Any expression of type `()`
may stand alone: a `match` whose arms all assert, an `if`, a block. Each ends
with `;`, the same as a call. Leave the `;` off a `match` and it reads as the
test body's result.

`Result` is must-use here as everywhere. No statement form drops one, so a test
source cannot hold an assertion it forgot to check.

## What a test can reach

A test source may import:

| | |
|---|---|
| The target under test | `//lib/money` for a library: its surface, the same name a dependent uses. `//cmd/server/main.buri` for a binary, whose entry point is a file and not a surface |
| The target's `dependencies` | The same libraries the target itself depends on |
| The suite's `test.dependencies` | Fakes, fixtures, matchers |
| `core/*` | Including the test platform: `core/testing/assert` and `core/host/testing` |
| Any test-only path | `//lib/ledger/testing`, `//lib/testing/fakes`. You declare the package in `test.dependencies` like any other library |

and may not:

- import a library-internal module. `from "//lib/money/cents.buri" import
  { toCents };` is an error, and this rule is what confines a test to the public
  surface;
- import another test source. The compiler compiles test sources independently,
  and nobody can name one as a module. Shared helpers belong in a library listed
  in `test.dependencies`;
- be imported by anything;
- `export` anything. A test source holds its `test` declarations and its private
  helpers, and nothing else.

```
error: lib/money/test/cents.buri imports a library-internal module
  --> lib/money/test/cents.buri:3:6
   |
 3 | from "//lib/money/cents.buri" import { toCents };
   |      ^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = tests reach their library the same way dependents do
   = import //lib/money, and re-export `toCents` from lib/money/lib.buri if it
     is part of the surface you meant to test
```

A test that needs an internal function has two cases. Either the function
belongs on the surface, so say so in `lib.buri` and everyone gets it. Or the
test asserts on an implementation detail, and it will break the next time that
detail changes.

## Test-only libraries

A helper more than one suite needs is not a test source. It is ordinary library
code that happens to be test-only, and it lives behind a path with a `testing`
segment. A `testing { sources: [...] }` block in the owning package's rule puts
it in the build. A consuming suite then names `//lib/ledger/testing` in its own
`test.dependencies`.

- `testing/lib.buri` decides that surface exactly as `lib.buri` one level up
  decides the library's. That includes `buri lint`, which reports an `export`
  the file does not carry as `dead-code`.
- Only a path with a `testing` segment may **export** a `context` declaration
  ([`language/programs.md` §11.3](../../language/programs.md)).
- [`libraries.md`](./libraries.md#the-testing-surface) says what those modules
  may import.

## Testing a binary

A binary's entry point is a **file**, not a surface, so its test sources import
`//cmd/server/main.buri`. Only that package's own test sources may. Everything
else matches testing a library. The suite sees what `main.buri` exports and
nothing more, so pushing logic behind the entry point is what makes it testable.

**`main` itself is not testable**, deliberately. It takes no parameters and
builds its own context out of `core/host`, so you have no fake to hand it
([`language/programs.md` §11](../../language/programs.md)). To assert on a
binary's failure modes, put them in a function taking an ordinary bounded `ctx`,
and call that function from a test with doubles of its own.

## The runner's context

`core/host/testing` is the platform a test source binds. It exports one double
per effect rather than one pre-assembled world, each real where it can be and
hermetic everywhere else. It is `core/host`'s surface written out for a test,
with the same names. Here you **call** them rather than refer to them, so
`clock()` answers a fresh clock every call and a test never inherits another
test's.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Alloc` | Real, with a per-test arena the runner reclaims. |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | Captured, and never printed; `captured()` reads either one back. |
| `stdin()` | `Stdin` | At end of input, so a suite never blocks on a pipe nobody is writing to. |
| `fs()` | `FsRead`, `FsWrite` | In-memory and empty. Writes are visible to that test and discarded after it. |
| `net()` | `Net` | **Refuses** every request with `.Refused`, until `respond` says what to answer. |
| `clock()` | `Clock` | At zero. `sleepMillis` advances it without sleeping. |
| `rand()` | `Rand` | Seeded at zero, so a failure reproduces. |
| `entropy()` | `Entropy` | Seeded at zero, on `rand()`'s own generator. It is the one double that does the *opposite* of what its effect promises a program, so an assertion can hold a token minted in a test. |
| `env()` | `Env` | No variables and no arguments. |
| `proc()` | `Proc` | **Absorbs** the exit instead of taking it, so the test carries on. |
| `tasks()` | `Tasks` | Runs the tasks one at a time, in **program order**, until a builder says otherwise. |
| `sockets()` | `Sockets` | Sockets with **no network** behind them: `open()` mints one, and what is pushed on it is recorded. |

You configure a double with a **method that answers a new handle**. A chain
reads in the order you apply it, and it leaves the value you called it on
unchanged:

| Builder | Answers |
|---|---|
| `clock().at(1000)` | A clock at that instant |
| `rand().seed(7)` | A generator at that seed, from the start of its sequence |
| `entropy().seed(7)` | The same, for `Entropy`. Literally the same sequence, so `crypto.randomBytes` and `random.bytes` at one seed answer alike |
| `env().variables([(Str, Str)])` | An environment with those variables and this one's arguments |
| `env().arguments([Str])` | An environment with those arguments and this one's variables |
| `stdin().lines([Str])` | A stream of those lines, then end of input |
| `stdin().bytes([U8])` | A stream of those octets, then end of input |
| `fs().files([(Str, Str)])` | A filesystem holding this one's files and these as well |
| `fs().filesBytes([(Str, [U8])])` | The byte twin, for a fixture that is not text |
| `fs().readOnly()` | The **same** files, through a handle whose every write fails with `.ReadOnly` |
| `net().respond(fn(Request) => Result<Response, NetError>)` | A network answering every request through that function |
| `tasks().anyOrder()`, `.everyOrder()`, `.seed(n)` | A scheduler running its tasks in one order, in every order, or in the one that seed names |

`fs()` is the one member answering **two** effects, because the filesystem is
two. A context that reads and writes binds the one double under both names:
`let disk = fs(); ... FsRead: disk, FsWrite: disk`. Two calls to `fs()` would be
two filesystems with nothing in common, so a named `context` declaration binds
only one half: its bindings are separate expressions with no `let` between them
to share a value.

`sockets()` has no builder. You mint a socket rather than declare one, and
`sockets().open()` is the method that does it.

The reader `Env` gives a program is `args(self): [Str]`, not `arguments`,
because a type's methods are one map keyed by name and this builder took that
name.

`lines` and `bytes` are the one pair that **replace** each other rather than
compose. A stream holds either the lines a test wrote or the octets it wrote. A
stdin built from octets answers `.None` to `readLine`, and the last builder in
the chain wins. `files` and `filesBytes` do compose, in either order, because
both write into the one map a file lives in.

`readOnly()` folds the `ReadOnly<C>` attenuation wrapper into a method. It
attenuates the *same* filesystem rather than a copy, so a read through the
attenuated handle answers whatever the filesystem holds now. Writing the wrapper
by hand still works, and it covers a case the method does not: the wrapper
attenuates any `FsWrite`, including one a test wrote.

### Reading the environment back

A test's outcome is the return value **plus the environment read back**.
`captured()` does that for a stream, and `TestFs` has two of its own:

| Read-back | Answers |
|---|---|
| `read(path)` | `Result<Str, IoError>`, what the filesystem holds there. The same answer `readFile` gives |
| `snapshot()` | `[(Str, Str)]`, every file, as text, **sorted by path** |
| `calls()` | `[FsCall]`, every call made through this handle, **in the order they completed** |

None of the three needs either half of the filesystem bound: asserting on what a
function wrote reads an environment back rather than performing an effect.
`snapshot()` sorts rather than reporting write order, so a function that
reorders two writes that do not interact still passes. It lists files only: a
directory `makeDir` created holds no octets, and `readDir` is the question it
answers.

`proc()` is the one double with nothing to read back. What a test asserts about
a function that exits is that the *test* carried on, and the assertions after
the call already say that. So `exitWith` absorbs the call and answers `()`.

Each call to a named context builds a fresh one. What one test writes to its
filesystem or prints to its captured stdout is invisible to the next.

### A network that answers

`net()` refuses everything. A test that reaches the network by accident says so
at its assertion rather than passing on an answer nobody wrote. `respond` hands
it a function, and that function is the fake server: it sees every `Request` the
code under test makes, and it either answers one or fails it.

```buri role=test
from "core/effect" import { Alloc, Net, NetError, Request };
from "core/host/testing" import { alloc, net };
from "core/net/http" import * as http;
# from "core/testing/assert" import * as assert;

# fn load<C: Net>(ctx: C, request: Request): Result<Int, NetError> {
#     http.send(ctx, request).map(fn(r) => r.status)
# }

test "a request nobody arranged for is refused rather than answered" {
    let ctx = context {
        Alloc: alloc(),
        Net: net(),
    };
    let asked = load(ctx, http.request(.Get, "https://example.test/a"));
    assert.eq(assert.err(asked), NetError.Refused);
}

test "the responder decides on the method and on a header" {
    let ctx = context {
        Alloc: alloc(),
    };
    let page = http.text(ctx, "Ledger");
    let server = net().respond(fn(request) => {
        let authorized = request.header("authorization") == .Some("Bearer t0ken");
        match ((request.method, authorized)) {
            (.Get, true) => .Ok(page),
            (_method, true) => .Ok(http.status(405)),
            (_any, false) => .Ok(http.status(401)),
        }
    });
    let live = context {
        Alloc: alloc(),
        Net: server,
    };
    let signed = http
        .request(.Get, "https://example.test/a")
        .withHeader(live, "authorization", "Bearer t0ken");
    assert.eq(assert.ok(load(live, signed)), 200);
    assert.eq(assert.ok(load(live, signed.withMethod(.Post))), 405);
    assert.eq(assert.ok(load(live, http.request(.Get, "https://example.test/a"))), 401);
}
```

Three things about that responder matter before you write one.

**It cannot take a context.** A lambda may not capture an effect-carrying value
([`language/effects.md` §10.6](../../language/effects.md)), so a responder
cannot call `http.text(ctx, ...)` inside itself. Build such a response *before*
the responder and capture it, the way `page` is captured above. A `Response` is
plain data, and anything that needs no allocation you can build inside, such as
`http.status(404)`.

**It answers a `Result`, so a test can fail the transport rather than the
server.** `.Err(.Timeout)`, `.Err(.Transport("socket closed"))` and the rest
reach the caller exactly as written, payload and all.

**`respond` replaces rather than composes.** It is one responder, not a routing
table. A responder that answers two URLs differently matches on
`request.path()`. Like every other builder here, it answers a **new** network
and leaves the one you called it on refusing.

Anything the runner does not provide is an ordinary struct with methods, since
effects are ordinary interfaces ([`language/effects.md`
§10.9](../../language/effects.md)). You bind it in a context exactly the way you
bind the runner's own implementations, and the guide has [a worked
one](../../guides/testing.md#writing-your-own).

A fake written this way answers from its fields rather than from a counter,
because it has no mutation to hold one in. `clock()`'s advancing clock and
`stdout()`'s accumulating buffer do change between calls, and that is a
privilege of the runner's own implementations: `core/host/testing` hands out no
way to open a slot in the runtime's tables.

### What the code under test asked for

`snapshot()` says what the world *is*. `calls()` says what the code **asked**
it. `TestFs`, `TestNet` and `TestStdin` each keep every call made through the
handle and answer them in the order they completed:

| Log | Answers |
|---|---|
| `fs().calls()` | `[FsCall]`, one per call to any of the twelve methods of `FsRead` and `FsWrite` |
| `net().calls()` | `[NetCall]`, one per request, whole: method, URL, headers and body |
| `stdin().calls()` | `[StdinCall]`, one per `readLine` or `readBytes`, with what it asked for |
| `sockets().sent()` | `[(Socket, Message)]`, one per message pushed, oldest first |

A test writes the call it expects with the constructor of the same name. These
are ordinary functions of `core/host/testing`: `readFile(path)`,
`writeFile(path, body)`, `renameFile(source, destination)`, `fetch(request)`,
`readBytes(n)`. There is one per method, and each takes the call's own
arguments. A path in one of them is the `Str` a `Path` spells. They derive `Eq`,
which an assertion compares, and `Show`, which a failing one prints.

```buri role=test
from "core/effect" import { Alloc, Net, NetError, Response };
# from "core/fs" import * as fs;
from "core/fs" import { FsRead };
from "core/host/testing" import { alloc, fetch, fs, net, readFile };
from "core/net/http" import * as http;
from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn cached<C: Alloc + FsRead + Net>(ctx: C, url: Str): Result<Response, NetError> {
#     match (fs.readText(ctx, path.of(ctx, "cache"))) {
#         .Ok(_body) => .Ok(http.status(200)),
#         .Err(_e) => http.get(ctx, url),
#     }
# }

test "a miss consults the cache once and then goes upstream" {
    let files = fs();
    let upstream = net().respond(fn(_request) => .Ok(http.status(200)));
    let ctx = context {
        Alloc: alloc(),
        FsRead: files,
        Net: upstream,
    };
    let _ = assert.ok(cached(ctx, "https://example.test/thing"));
    assert.eq(files.calls(), [readFile("cache")]);
    assert.eq(upstream.calls(), [
        fetch(http.request(.Get, "https://example.test/thing")),
    ]);
}

test "a hit never reaches the network at all" {
    let files = fs().files([("cache", "hit")]);
    let upstream = net();
    let ctx = context {
        Alloc: alloc(),
        FsRead: files,
        Net: upstream,
    };
    let _ = assert.ok(cached(ctx, "https://example.test/thing"));
    assert.eq(upstream.calls(), []);
}
```

Four things about the log.

**A call that failed is a call.** A read that found nothing and a write refused
through `readOnly()` are both in the log. The log holds what the code asked, and
the test already has the answer in the return value.

**Reading the environment back is not a call.** `read`, `snapshot`, `captured`
and `calls` itself ask the *fixture* a question, so none of them appears.

**The log is per handle.** Every builder answers a new double with a log of its
own, `readOnly()` and `respond` included.

**The log records octets as the text they spell.**
`writeFileBytes("b", [104, 105])` reads back as a call whose body is `"hi"`, and
`writeFileBytes(path, body)` is the constructor that writes it down.

### What breaks: the fault plan

`files` and `respond` say what a call *finds*. `faults` says what a call **fails
with**. Those are the only two sources, so a reader knows which half of a test
to look in.

A fault is one of the `Call` constructors above and an error. `fails(e)` fails
every matching call. `failsOnCall(n, e)` fails the `n`th of them, counted from
one over the *matching* calls, so a read between two writes does not move the
number. Matching uses the `Eq` those records derive, so you spell a fault
exactly as `calls()` reports the call it names.

```buri role=test
from "core/effect" import { Alloc, IoError };
# from "core/fs" import * as fs;
from "core/fs" import { FsWrite, Path };
from "core/host/testing" import { alloc, appendFile, fs };
from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn commit<C: Alloc + FsWrite>(
#     ctx: C,
#     at: Path,
#     entries: [[U8]],
#     i: Int,
# ): Result<(), IoError> {
#     match (entries.get(i)) {
#         .None => .Ok(()),
#         .Some(entry) => {
#             match (fs.append(ctx, at, entry)) {
#                 .Err(e) => .Err(e),
#                 .Ok(_written) => commit(ctx, at, entries, i + 1),
#             }
#         },
#     }
# }

test "the third append fails and nothing after it is written" {
    // `commit` writes and never reads, so the context binds `FsWrite` alone —
    // and the read-back below needs no effect at all.
    let wal = fs().faults([
        appendFile("wal", [99]).failsOnCall(1, .Other("disk full")),
    ]);
    let ctx = context {
        Alloc: alloc(),
        FsWrite: wal,
    };
    let at = path.of(ctx, "wal");
    assert.eq(assert.err(commit(ctx, at, [[97], [98], [99]], 0)), .Other("disk full"));
    assert.eq(assert.ok(wal.read("wal")), "ab");
}
```

Three things follow from a fault being a value rather than a moment.

**A call the plan fails never happens, and is still a call.** Nothing is written
and nothing is removed, and `calls()` still has it.

**The error is the value the test wrote.** `.Other("disk full")` arrives with
its text, and so do `NetError`'s `.BadUrl` and `.Transport`.

**A fault whose call never happens fails the test.** A plan claims something
about what the code under test does, and nothing exercises an unused claim. The
runner checks the plan at the end of every block, so an unused fault is a
failure naming itself:

```text
FAIL //lib/journal  test/journal.buri  "a fault whose call never happens fails the test"
  a fault was planned and never happened: readFile("log") fails .NotFound
```

`faults` is a builder like every other one here. It answers a **new** double,
with a log of its own, over the same files, and it **replaces** rather than
composes, as `respond` does.

### The step boundary, which the plan does not replace

A fault fails a *call*. It says nothing about the state a program sits in
between two of them. How you split the code decides whether a test can reach
that boundary, and no part of the plan can express it ([the
guide](../../guides/testing.md#what-breaks-fault-plans) has the shape). Keep an
effectful step to a single call and a fault plan says exactly what it looks like
it says.

This is defence in depth. The primary mechanism is that a test whose call never
passed a `Net`-bounded context cannot open a socket in anything it transitively
calls ([`language/effects.md` §10](../../language/effects.md)). There is no
third layer: the toolchain applies no operating-system confinement, because a
suite has no name for a real capability to begin with
([`hermeticity.md`](./hermeticity.md)).

### The order the work happens in

`Tasks.parallel` promises its **results** in the items' order. It promises
nothing about the order the work runs in. That second order is the one thing
about a concurrent program a test cannot otherwise pin down, so `tasks()` makes
it a value the test writes:

| Builder | Answers |
|---|---|
| `tasks()` | A scheduler running its tasks in **program order**, the items' own |
| `tasks().anyOrder()` | One seeded order per program: the one this suite's **content** names, unless a seed says otherwise |
| `tasks().seed(n)` | The order numbered `n`, counted from zero, wrapping past the last |
| `tasks().everyOrder()` | Every order: the whole `test` body runs once per completion order |
| `tasks().faults([TaskFault])` | The tasks the plan names end the block, with the reason the test gave |

The same builders decide the order `tasks.spawn`'s work runs in, because a
scope runs a round of what was spawned through `Tasks.parallel`. So a test of
background work names an order too, and waits on no real time.

Nothing here is concurrent. A task runs to completion before the next one
starts, and `calls()` reports them in the order they finished:

```buri repo=cli/tests/example role=test
# from "core/effect" import { Alloc, Tasks };
# from "core/host/testing" import { alloc, task, tasks };
# from "core/tasks" import * as tasks;
# from "core/testing/assert" import * as assert;

# fn doubled<C: Alloc + Tasks>(ctx: C, items: [Int]): [Int] {
#     tasks.parallel(ctx, items, fn(_c, _i, item) => item * 2)
# }

test "the answer does not depend on the order the work finished in" {
    // `seed(5)` is the last of the six orders of three tasks — the reverse of
    // program order. Named rather than reached through `anyOrder()`, because a
    // block that asserts on the order has to name the order it means.
    let scheduler = tasks().seed(5);
    let ctx = context {
        Alloc: alloc(),
        Tasks: scheduler,
    };
    // The items' order, whatever order the work ran in.
    assert.eq(doubled(ctx, [1, 2, 3]), [2, 4, 6]);
    // And the order it ran in, which is the thing this double chose.
    assert.eq(scheduler.calls(), [task(2), task(1), task(0)]);
}
```

**A seed is the order's own number.** `n` tasks have `n!` orders, and a seed
says which of them, counted from zero in the order the orders themselves sort
in. So `seed(0)` is program order and the last seed is the reverse. A seed
*replays* rather than merely re-randomising: `everyOrder`'s fourth run and
`seed(3)` are the same order.

**`anyOrder()` with no seed is the order this suite's own content names**, and
it is deliberately not random. The seed comes from the suite's action key, so
the order a suite schedules in changes exactly when the verdict that order
produced stops being reusable, and never on a run that changed nothing. A random
seed would poison the cache: a suite passes under one order, gets remembered as
passing, then re-runs under another. The report names the order it ran in and
the seed that replays it.

Use `anyOrder()` to *find* an order that breaks a program, and `seed(n)` to keep
it.

**`everyOrder()` re-runs the body, not the fan-out.** Re-running only the loop
would re-run a task's effects against a filesystem the last order had already
written to. So every run builds its own doubles from the same lines, and the
assertion at the end of the body asserts about *every* order. `runs()` says
which run this is, counted from one, and `orders()` says how many there will be.
Six tasks make 720 runs of the block; above six it refuses and says so. The
first failing order ends the block, so the orders after it never run.

**A fault ends the block.** A task has no error channel. `parallel` answers
`[B]` and every `B` comes from the closure, so a task the plan fails ends the
program, exactly as a task that died for real would. The tasks scheduled before
it have run and had their effects, and the ones after it never start.
`task(k).fails(why)` fails that task every time something reaches it, and
`task(k).failsOnCall(n, why)` fails the `n`th, counted over the fan-outs that
reach it. As everywhere else here, **a fault whose task is never reached fails
the test**.

### A socket with no server

A `Socket` is inert. It is one number a program may hold, put in a list and send
to an actor, and its two methods need `Sockets` and nothing else. So a test can
run the half of a WebSocket program that *pushes* on its own, against
`sockets()`.

| Member | Answers |
|---|---|
| `sockets().open()` | A `Socket` of that double's, open, with nothing behind it |
| `sockets().sent()` | `[(Socket, Message)]`, every push, oldest first, both framings in one list |
| `sockets().isOpen(socket)` | Whether that socket is still one this double will take a message for |

```buri role=test
# from "core/effect" import { Sockets };
# from "core/host/testing" import { sockets };
# from "core/net/server" import { Message, Socket };
# from "core/testing/assert" import * as assert;

# fn broadcast<C: Sockets>(ctx: C, room: [Socket], said: Message): () {
#     room.foldCtx(ctx, fn(c, _sofar, socket) => socket.send(c, said), ())
# }

test "everybody in the room hears it" {
    let wire = sockets();
    let ctx = context {
        Sockets: wire,
    };
    let first = wire.open();
    let second = wire.open();
    let _said = broadcast(ctx, [first, second], .Text("hello"));
    // The socket that did not publish heard it, which is the whole of what a
    // broadcast is — and there is no listener, no port and no client here.
    assert.eq(wire.sent(), [
        (first, Message.Text("hello")),
        (second, Message.Text("hello")),
    ]);
}
```

**A message to a socket that has gone is dropped.** That is the real platform's
rule rather than the double's: `send` never waits, so this side could never
answer "did this arrive". Three things drop alike: a socket this double closed,
a handle a program invented, and a socket another `sockets()` minted. Two
doubles are two worlds, the way two `fs()` calls are two filesystems. `isOpen`
asks about *one* socket rather than counting the open ones, because the blocks
of a suite share a runner. The double keeps neither the close's code nor its
phrase, for `proc()`'s reason.

**Reading a socket is not here, and deliberately.** That authority belongs to
`Listen`, which means whoever holds the listener, and what a fake acceptor
answers is the test's own decision. What a test *cannot* write for itself is a
double that records: an effect method takes only `self`, `self` is immutable,
and so a hand-written `Sockets` has nowhere to put what it was told.

## Test data and golden files

You write a suite's filesystem in the suite, with `fs().files([...])`. You write
a golden value in the suite's own source, where an editor rewrites it rather
than the runner.

There was a `test { data: [...] }` field and a `buri test --accept` that rewrote
what it named. Both are retired ([`buri docs error
retired-test-data`](../errors/retired-test-data.md)), and holding a golden no
longer costs a suite a backend.

## Running

A suite runs as a native binary for the host unless something sends it to
JavaScript: its own `test { platforms }`, `--output=js`, or the fallback for a
toolchain that cannot build one (`buri docs cli test`). The fallback prints one
line on standard error per suite. It never changes what the suite means, because
both backends owe the same answers ([`tags.md`](./tags.md#tags-and-tests)). A
*program* the native backend has no body for is refused rather than rerouted.

The toolchain compiles the suites that run natively into one binary per
tag-compatible batch, and links it once
([`tags.md`](./tags.md#one-binary-for-several-suites) has the policy). It
changes nothing you see: the runner still caches verdicts one suite at a time,
reports one suite at a time, and runs a suite that cannot batch on its own.

Output names the target, the file, and the test:

```
FAIL //lib/money  test/cents.buri  "pads the cents place"
  assert.eq failed
    actual:   "$19.5"
    expected: "$19.05"
  --> lib/money/test/cents.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

That line also counts a suite that never *compiled*. It has no cases to pass or
fail, so it gets a clause of its own, and the clause appears only when the count
is not zero:

```
0 passed, 0 failed, 0 skipped, 1 failed to compile (0.0s)
```

The diagnostic saying why goes to stderr and the summary to stdout. The runner
flushes stderr before writing the summary, so a log with a broken suite in it
never opens with a line that looks like a clean run.

Tests are ordinary build actions. A suite whose sources, target, dependencies
and toolchain are unchanged does not re-run, and reports as cached. Buri has no
mutable global state, no ambient I/O and no observable ordering, so the runner
is free to shard across processes and to run a suite's tests in any order. No
part of a suite's result may depend on that freedom, so there is no flag to turn
it off. A suite that needs one has a dependency it has not admitted to.
