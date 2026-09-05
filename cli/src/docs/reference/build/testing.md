# Test targets and the testing host

Tests live inside the target they test, are declared in its build rule, and can
reach only what a dependent could reach. There is no separate test target, no
test-only source directory outside the package, and no way to test a private
function directly.

This page is the exact rules: the `test` block, what a test source may import,
every member of `core/host/testing`, and what a run does with a suite. To learn
to write one from scratch: [testing your code](../../guides/testing.md).

The language side — the `test` declaration, `core/testing/assert`, and the
`context` form — is [`language/programs.md` §11.2 and
§11.3](../../language/programs.md).

## The `test` block

```textproto schema=build
library {
    sources: ["cents.buri", "parse.buri"]

    test {
        sources: ["test/cents.buri", "test/parse.buri"]
    }
}
```

A module listed in a `test.sources` is a **test source**. That is the only thing
that makes one: `test` declarations and imports of test-only modules are legal
there and nowhere else, and the file is compiled into a test binary rather than
into the library.

- A `test` declaration takes no parameters and returns nothing. It passes unless
  an assertion in it fails, and a failing assertion ends that test and no other.
- **A title is used once per file.** Two tests in one module sharing one are a
  compile error (`duplicate-test-name`), because a title is how a failure is
  reported and how `--filter` picks one out, and two that share it in one file
  cannot be told apart. Two files of one suite may share a title: they are
  separate modules, and each failure names its own file and its own line.
- A test source and `main`'s body are the only places in the language where a
  context is **created** rather than received, which is why `core/host/testing`
  is importable only from a test source.

A test source is code, so [`buri lint`](../cli/lint.md#what-it-reads) holds it to
every rule a library source is held to, and `--fix` rewrites it the same way.

`core/testing/assert` is an ordinary module — the name `assert` comes from
`import * as assert`, and nothing stops a file calling it something else.

| Function | Answers |
|---|---|
| `assert.eq`, `assert.notEq`, `assert.isTrue`, `assert.isFalse`, `assert.contains`, `assert.isEmpty`, `assert.notEmpty`, `assert.len`, `assert.gt`, `assert.ge`, `assert.lt`, `assert.le`, `assert.approxEq` | `()`, so the call stands alone as a statement |
| `assert.ok`, `assert.err`, `assert.some` | The unwrapped value |

The statement rule asks for the type and not the shape: any expression of type
`()` may stand alone — a `match` whose arms all assert, an `if`, a block — and
each is terminated by `;`, the same as a call. Leave the `;` off a `match` and it
reads as the test body's result, which is what the block would have returned had
anything followed it.

`Result` is must-use here as everywhere, so an assertion a test forgets to check
is not something a test source can contain: there is no statement form that
drops one.

## What a test can reach

A test source may import:

| | |
|---|---|
| The target under test | `//lib/money` for a library — its surface, the same name a dependent uses — and `//cmd/server/main.buri` for a binary, whose entry point is a file and not a surface |
| The target's `dependencies` | The same libraries the target itself depends on |
| The suite's `test.dependencies` | Fakes, fixtures, matchers |
| `core/*` | Including the test platform: `core/testing/assert` and `core/host/testing` |
| Any test-only path | `//lib/ledger/testing`, `//lib/testing/fakes` — the package is declared in `test.dependencies` like any other library |

and may not:

- import a library-internal module — `from "//lib/money/cents.buri" import
  { toCents };` is an error, and this is the rule that confines a test to the
  public surface;
- import another test source — test sources are compiled independently and are
  not modules anybody can name. Shared helpers belong in a library listed in
  `test.dependencies`;
- be imported by anything;
- `export` anything. A test source's items are its `test` declarations and its
  private helpers.

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

That error is the whole design in one message. If a test needs an internal
function, either the function belongs on the surface — in which case say so in
`lib.buri` and everyone gets it — or the test is asserting on an implementation
detail and will break the next time the detail changes.

## Test-only libraries

A helper more than one suite needs is not a test source: it is ordinary library
code that happens to be test-only, and it lives behind a path with a `testing`
segment. A `testing { sources: [...] }` block in the owning package's rule puts
it in the build, and a consuming suite names `//lib/ledger/testing` in its own
`test.dependencies`, the same label mechanism as any other dependency.

- `testing/lib.buri` decides that surface exactly as `lib.buri` decides the
  library's one level up — including for `buri lint`, which reports an `export`
  that file does not carry as `dead-code` and leaves alone the shapes it does.
- A `context` declaration may be **exported** only from a path with a `testing`
  segment ([`language/programs.md` §11.3](../../language/programs.md)).
- What those modules may import, and why the restriction is carried by the path
  rather than by a `testonly` field, is
  [`libraries.md`](./libraries.md#the-testing-surface).

## Testing a binary

A binary's entry point is a **file** and not a surface, so its test sources
import `//cmd/server/main.buri` — and only that package's own test sources may,
which is the rule that keeps `//lib/money/cents.buri` internal, one directory
over. Everything else is identical to testing a library: the suite sees what
`main.buri` exports and nothing more, so pushing logic behind the entry point is
what makes it testable.

**`main` itself is not testable**, and that is deliberate. It takes no
parameters and builds its own context out of `core/host`, so there is no fake to
hand it ([`language/programs.md` §11](../../language/programs.md)). A binary
whose failure modes are to be asserted on puts them in a function taking an
ordinary bounded `ctx`, which a test then calls with doubles of its own — and
`main`'s context has to bind every effect that function reaches for, so the list
is the same in both directions.

## The runner's context

`core/host/testing` is the platform a test source binds, and it exports one
double per effect rather than one pre-assembled world — each real where it can
be and hermetic everywhere else. It is `core/host`'s surface written out for a
test: the same names — `alloc`, `stdout`, `stderr`, `stdin`, `fs`, `net`,
`clock`, `rand`, `entropy`, `env`, `proc` — **called** rather than referred to.
`core/host`'s `clock` is one clock because a process has one; `clock()` is a
fresh clock every call, so a test never inherits another test's.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Alloc` | Real, with a per-test arena the runner reclaims. |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | Captured, and never printed; `captured()` reads either one back. |
| `stdin()` | `Stdin` | At end of input, so a suite never blocks on a pipe nobody is writing to. |
| `fs()` | `FsRead`, `FsWrite` | In-memory and empty. Writes are visible to that test and discarded after it. |
| `net()` | `Net` | **Refuses** every request with `.Refused`, until `respond` says what to answer. |
| `clock()` | `Clock` | At zero. `sleepMillis` advances it without sleeping. |
| `rand()` | `Rand` | Seeded at zero, so a failure reproduces. |
| `entropy()` | `Entropy` | Seeded at zero, on `rand()`'s own generator — the one double that is the *opposite* of what its effect promises a program, so a token minted in a test is a value an assertion can hold. |
| `env()` | `Env` | No variables and no arguments. |
| `proc()` | `Proc` | **Absorbs** the exit instead of taking it, so the test carries on. |
| `tasks()` | `Tasks` | Runs the tasks one at a time, in **program order**, until a builder says otherwise. |
| `sockets()` | `Sockets` | Sockets with **no network** behind them: `open()` mints one, and what is pushed on it is recorded. |

Configuration is a **method on the value that answers a new handle**, so a
chain reads in the order it is applied and the value it was called on is
unchanged:

| Builder | Answers |
|---|---|
| `clock().at(1000)` | A clock at that instant |
| `rand().seed(7)` | A generator at that seed, from the start of its sequence |
| `entropy().seed(7)` | The same, for `Entropy` — and literally the same sequence, so `crypto.randomBytes` and `random.bytes` at one seed are one answer |
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
two: a context that reads and writes binds the one double under both names —
`let disk = fs(); ... FsRead: disk, FsWrite: disk` — and two calls to `fs()`
would be two filesystems with nothing in common. A named `context` declaration
therefore binds only one half, since its bindings are separate expressions with
no `let` between them to share a value.

`sockets()` has no builder, because there is nothing to configure: a socket is
minted rather than declared, and `sockets().open()` is the method that does it.

Every builder here is spelled as the design note writes it, and `arguments` is
the one that cost something to get. A type's methods are one map keyed by name,
and a method written in `impl Env for TestEnv` goes into it beside the ones
written in `impl TestEnv`, so neither the extra argument nor the different
return type tells the two apart: `Env`'s reader and this builder could not both
be `arguments`. The **reader** is what moved. `core/effect`'s `Env` declares
`args(self): [Str]` now — the name a program already used, since `core/env` has
always exported `args(ctx)` and was the method's only caller — and the builder
on the double is `arguments`.

`lines` and `bytes` are the one pair that **replace** each other rather than
composing: a stream is either the lines a test wrote or the octets it wrote, a
stdin built from octets answers `.None` to `readLine`, and the last builder in
the chain is the stream. `files` and `filesBytes` do compose, in either order,
because both write into the one map a file lives in.

`readOnly()` is the `ReadOnly<C>` attenuation wrapper folded into a method, and
the fold keeps what made it a wrapper: it attenuates the *same* filesystem
rather than a copy, so a read through the attenuated handle answers whatever the
filesystem holds now. Writing the wrapper by hand still works, and still has a
use the method does not cover — it attenuates any `FsWrite`, including one a
test wrote, and the method attenuates only this one.

### Reading the environment back

The outcome of a test is the return value **plus the environment read back**.
`captured()` does that for a stream, and `TestFs` has two of its own:

| Read-back | Answers |
|---|---|
| `read(path)` | `Result<Str, IoError>` — what the filesystem holds there, the same answer `readFile` gives |
| `snapshot()` | `[(Str, Str)]` — every file, as text, **sorted by path** |
| `calls()` | `[FsCall]` — every call made through this handle, **in the order they completed** |

`faults([...])` is the other half of a fixture and has its own section below:
what a call finds comes from `files`, and what a call fails with comes from
there.

None of the three needs either half of the filesystem bound: asserting on what a
function wrote is
reading an environment back rather than performing an effect. `snapshot()` is
sorted rather than in write order so that a function which reorders two writes
that do not interact does not fail the test, and it lists files only — a
directory `makeDir` created holds no octets, and `readDir` is the question it
answers.

`proc()` is the one double with nothing to read back, and deliberately: what a
test asserts about a function that exits is that the *test* carried on, which
the assertions written after the call already say. An exit code recorded where
no method could read it would be state kept for its own sake, so `exitWith`
absorbs the call and answers `()`.

Each call to a named context builds a fresh one, so what one test writes to its
filesystem or prints to its captured stdout is invisible to the next. That is
why a named context is called rather than referred to.

### A network that answers

`net()` refuses everything, and that is the default worth having: a test that
reaches the network by accident says so at its assertion rather than passing on
an answer nobody wrote. `respond` hands it a function, and that function is the
fake server — it is given every `Request` the code under test makes, and either
answers it or fails it.

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

Three things about that responder are worth knowing before writing one.

**It cannot take a context.** A lambda may not capture an effect-carrying value
([`language/effects.md` §10.6](../../language/effects.md)), so a responder
cannot call `http.text(ctx, ...)` inside itself — which is precisely what makes
a `fn(Request) => Result<Response, NetError>` a pure function of the request and
safe to hold in a value. Build such a response *before* the responder and
capture it, the way `page` is captured above: a `Response` is plain data.
Anything that needs no allocation — `http.status(404)`, or a `Response` literal,
since a list literal needs no context — can be built inside.

**It answers a `Result`, so a test can fail the transport rather than the
server.** `.Err(.Timeout)`, `.Err(.Transport("socket closed"))` and the rest
reach the caller exactly as written, payload and all.

**`respond` replaces rather than composing.** It is one responder and not a
routing table: a responder that answers two URLs differently matches on
`request.path()`, and two responders for one request would have no answer to
which of them wins. Like every other builder here it answers a **new** network
and leaves the one it was called on refusing.

`net()` is also the one double whose configuration is the value itself rather
than a handle into the runner's table. Every other one holds *state* — a
transcript that grows, a clock that advances — and state is what a runner can
keep on a program's behalf. A responder is *behaviour*, and behaviour is what it
cannot keep: a function value is a code pointer and an environment, and for the
runtime to invoke one it would have to call back into compiled Buri. So `fetch`
is written in Buri and calls the responder directly, and `Net` has no row in
either runtime table.

Anything the runner does not provide is an ordinary struct with methods, since
effects are ordinary interfaces ([`language/effects.md`
§10.9](../../language/effects.md)), and it is bound in a context exactly the way
the runner's own implementations are — the guide has [a worked
one](../../guides/testing.md#writing-your-own). For `Net` in a test source,
`net().respond` is that same thing already written: a `fetch` method and a
responder are the same function of the same request.

A fake written this way answers from its fields rather than from a counter,
because there is no mutation to hold one in. `clock()`'s advancing clock and
`stdout()`'s accumulating buffer do change between calls, and that is a
privilege of the runner's own implementations rather than a mechanism a fake can
borrow: each of those constructors is an intrinsic that installs a slot in a
table the runtime owns, and `core/host/testing` hands out no way to open one.

### What the code under test asked for

`snapshot()` says what the world *is*; `calls()` says what it was **asked**.
`TestFs`, `TestNet` and `TestStdin` each keep every call made through the handle
and answer them in the order they completed:

| Log | Answers |
|---|---|
| `fs().calls()` | `[FsCall]` — one per call to any of the twelve methods of `FsRead` and `FsWrite` |
| `net().calls()` | `[NetCall]` — one per request, whole: method, URL, headers and body |
| `stdin().calls()` | `[StdinCall]` — one per `readLine` or `readBytes`, with what it asked for |
| `sockets().sent()` | `[(Socket, Message)]` — one per message pushed, oldest first |

A test writes the call it expects with the constructor of the same name, and
these are ordinary functions of `core/host/testing`: `readFile(path)`,
`writeFile(path, body)`, `renameFile(source, destination)`, `fetch(request)`,
`readBytes(n)` — one per method, taking the call's own arguments. A path in one
of them is the `Str` a `Path` spells, which is what `text()` answers and what a
`FsCall` records. They derive
`Eq`, which is what an assertion compares, and `Show`, which is what a failing
one prints.

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

Four things are worth knowing about the log.

**A call that failed is a call.** A read that found nothing and a write refused
through `readOnly()` are both in it: the log is what was asked, and the answer
is the return value the test already has.

**Reading the environment back is not a call.** `read`, `snapshot`, `captured`
and `calls` itself ask the *fixture* a question rather than asking the double
for anything, so none of them appears.

**The log is per handle.** Every builder answers a new double with a log of its
own, `readOnly()` and `respond` included — so the calls a test reads back are
the ones made through the value it put in the context.

**Octets are recorded as the text they spell**, which is `snapshot()`'s rule and
is there for `snapshot()`'s reason: `writeFileBytes("b", [104, 105])` reads back
as a call whose body is `"hi"`, and `writeFileBytes(path, body)` is the
constructor that writes it down.

### What breaks: the fault plan

`files` and `respond` say what a call *finds*. `faults` says what a call
**fails with** — and those are the only two sources: success comes from the
environment and failure comes from the plan, so a reader of a test knows which
half of it to look in.

A fault is one of the `Call` constructors above and an error. `fails(e)` fails
every matching call; `failsOnCall(n, e)` fails the `n`th of them, counted from
one over the *matching* calls, so a read between two writes does not move the
number. Matching is the `Eq` those records derive, which is what makes a fault
readable: it is spelled exactly as `calls()` reports the call it names.

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

**A call the plan fails is not performed, and is still a call.** Nothing is
written and nothing is removed, and `calls()` has it — because the code under
test asked the filesystem for something and was answered.

**The error is the value the test wrote.** `.Other("disk full")` arrives with its
text, and so do `NetError`'s `.BadUrl` and `.Transport`. The plan travels in the
program rather than in the runner for exactly that reason.

**A fault whose call never happens fails the test.** A plan is a claim about what
the code under test does, and a claim nothing exercised is one the next change to
that code will quietly stop being true. The runner checks it at the end of every
block, so an unused fault is a failure naming itself:

```text
FAIL //lib/journal  test/journal.buri  "a fault whose call never happens fails the test"
  a fault was planned and never happened: readFile("log") fails .NotFound
```

`faults` is a builder like every other one here: it answers a **new** double,
with a log of its own, over the same files. It **replaces** rather than
composing, as `respond` does — two plans for one call have no answer to which of
them wins — and the plan it replaced is retired, promise and all.

### The step boundary, which the plan does not replace

A fault fails a *call*. It says nothing about the state a program is left in
between two of them, and reaching that boundary from a test is a matter of how
the code is split rather than of anything the plan can express
([the guide](../../guides/testing.md#what-breaks-fault-plans) has the shape).
Keeping an effectful step to a single call is what makes a fault plan say
exactly what it looks like it says: a step that writes once is a step whose
failure has one meaning, and one fault names it.

This is defence in depth rather than the primary mechanism. The primary
mechanism is that a test whose call never passed a `Net`-bounded context cannot
open a socket in anything it transitively calls — that is [`language/effects.md`
§10](../../language/effects.md), not a build system feature. There is no third
layer: the toolchain applies no operating-system confinement, because a suite
has no name for a real capability to begin with
([`hermeticity.md`](./hermeticity.md)).

### The order the work happens in

`Tasks.parallel` promises its **results** in the items' order and promises
nothing about the order the work runs in. That second order is the one thing
about a concurrent program a test cannot otherwise pin down, so `tasks()` makes
it a value the test writes:

| Builder | Answers |
|---|---|
| `tasks()` | A scheduler running its tasks in **program order** — the items' own |
| `tasks().anyOrder()` | One seeded order per program: the one this suite's **content** names, unless a seed says otherwise |
| `tasks().seed(n)` | The order numbered `n`, counted from zero, wrapping past the last |
| `tasks().everyOrder()` | Every order — the whole `test` body runs once per completion order |
| `tasks().faults([TaskFault])` | The tasks the plan names end the block, with the reason the test gave |

Nothing here is concurrent. A double that raced would be the thing it exists to
remove, so a task runs to completion before the next one starts and `calls()`
reports them in the order they finished:

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

**A seed is the order's own number.** There are `n!` orders of `n` tasks and a
seed is which of them, counted from zero in the order the orders themselves
sort in — so `seed(0)` is program order, the last seed is the reverse, and a
seed *replays* rather than merely re-randomising. `everyOrder`'s fourth run and
`seed(3)` are the same order, which is what lets a failure name one line to
paste back.

**`anyOrder()` with no seed is the order this suite's own content names**, and
it is deliberately not random. The seed is derived from the suite's action key
— the hash of every source in its closure that `buri` already keys the result
cache on — so the order a suite schedules in changes exactly when the verdict
that order produced stops being reusable, and never on a run that changed
nothing. A random seed would be the shape that poisons the cache: a suite that
passed under one order and was remembered as passing, re-running under another.
And a failure nobody can reproduce is a failure nobody fixes, which is why the
report names the order it ran in and the seed that replays it.

A block asserting on the order it ran in should name one that way too:
`anyOrder()` is for *finding* an order that breaks a program, and `seed(n)` is
for keeping it.

**`everyOrder()` re-runs the body, not the fan-out.** A task's effects are the
point, and re-running only the loop would re-run them against a filesystem the
last order had already written to — so every run builds its own doubles from the
same lines, and the assertion at the end of the body is an assertion about
*every* order. `runs()` says which run this is, counted from one, and `orders()`
how many there will be. Six tasks are 720 runs of the block; above six it
refuses and says so, because a fan-out that wide is `anyOrder`'s question.

The first failing order ends the block — a failed assertion is an abort and
there is nothing to catch — so the orders after it do not run.

**A fault ends the block.** A task has no error channel: `parallel` answers
`[B]` and every `B` comes from the closure, so a task the plan fails is a task
that ends the program, which is what a task that died would do to the run for
real. The tasks scheduled before it have run and had their effects; the ones
after it have not started. `task(k).fails(why)` fails that task every time it is
reached and `task(k).failsOnCall(n, why)` the `n`th, counted over the fan-outs
that reach it — and, as everywhere else here, **a fault whose task is never
reached fails the test**.

### A socket with no server

A `Socket` is inert — one number a program may hold, put in a list and send to
an actor — and its two methods need `Sockets` and nothing else. So the half of a
WebSocket program that *pushes* is a half a test can run on its own, and
`sockets()` is what it runs against: `open()` mints a socket with no network
behind it, and `sent()` reads back every message that was pushed on one.

| Member | Answers |
|---|---|
| `sockets().open()` | A `Socket` of that double's, open, with nothing behind it |
| `sockets().sent()` | `[(Socket, Message)]` — every push, oldest first, both framings in one list |
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

**A message to a socket that has gone is dropped**, which is the real platform's
rule rather than the double's: `send` never waits, so "did this arrive" was
never a question this side could answer. Three things are dropped alike — a
socket this double closed, a handle a program invented, and a socket another
`sockets()` minted, because two doubles are two worlds the way two `fs()` calls
are two filesystems. `isOpen` is the question asked directly, and it is a
question about *one* socket rather than a count of the open ones: the blocks of
a suite share a runner, so a count would answer differently depending on what
ran beside it.

The close's code and phrase are not kept, for `proc()`'s reason: they are what
the far side would be told, there is no far side, and a number held where
nothing can read it is state kept for its own sake.

**Reading a socket is not here, and deliberately.** That authority is `Listen`'s
— it belongs to whoever holds the listener — and what a fake acceptor answers is
the test's own decision, so a test writes one for itself as it writes any other
fake. What a test *cannot* write for itself is a double that records: an effect
method takes only `self`, `self` is immutable, and so a hand-written `Sockets`
has nowhere to put what it was told. That is the whole of why this one is here.

## Test data and golden files

A suite's filesystem is written in the suite, with `fs().files([...])`, and a
golden value is written in the suite's own source, where an editor rather than
the runner is what rewrites one.

**There was a `test { data: [...] }` field, and there was a `buri test --accept`
that rewrote what it named.** The field listed files on disk; the *runner* read
them and handed the suite their contents. That made a suite's filesystem a fact
about the build rather than about the program, and it could only be told to a
suite the toolchain ran under a runner — a linked test binary has none, so
`data()` was empty there and a declared file read `.Err(.NotFound)` where
`buri test` read its contents. The toolchain hid that by sending every suite
that declared `data` back to JavaScript: one build-file field deciding which
backend a program was allowed to run on. Both are retired
([`buri docs error retired-test-data`](../errors/retired-test-data.md)), and no
suite is refused a backend for holding a golden.

## Running

A suite runs as a native binary for the host unless something sends it to
JavaScript: its own `test { platforms }`, `--output=js`, or the fallback for a
toolchain that cannot build one (`buri docs cli test`). The
fallback prints one line on standard error per suite; it never changes what the
suite means, because the two backends are held to the same answers
([`tags.md`](./tags.md#tags-and-tests)). A *program* the native backend has no
body for is refused rather than rerouted, because rerouting it would be
answering with the backend that was not asked.

The suites that run natively are compiled into one binary per tag-compatible
batch and linked once, because a small suite's cost is the link and the first
execution rather than the compile
([`tags.md`](./tags.md#one-binary-for-several-suites) has the
policy). It changes nothing a reader sees: verdicts are still cached one suite at
a time, reported one suite at a time, and a suite that cannot batch runs on its
own.

Output names the target, the file, and the test:

```
FAIL //lib/money  test/cents.buri  "pads the cents place"
  assert.eq failed
    actual:   "$19.5"
    expected: "$19.05"
  --> lib/money/test/cents.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

A suite that never *compiled* is in that line as well. It has no cases to pass
or fail, so it gets a clause of its own — present only when the count is not
zero, the way the cached note is:

```
0 passed, 0 failed, 0 skipped, 1 failed to compile (0.0s)
```

The diagnostic saying why goes to stderr and the summary to stdout, and stderr
is flushed before the summary is written: a log with a broken suite in it never
opens with a line that looks like a clean run.

Tests are ordinary build actions: a suite whose sources, target, dependencies,
and toolchain are unchanged is not re-run, and reports as cached. Because there
is no mutable global state, no ambient I/O, and no observable ordering, the
runner is free to shard across processes and to run a suite's tests in any
order. Nothing about a suite's result may depend on that freedom, so there is
no flag to turn it off — a suite that needs one is a suite with a dependency it
has not admitted to.
