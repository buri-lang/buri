# Testing

Tests live inside the target they test, are declared in its build rule, and can
reach only what a dependent could reach. There is no separate test target, no
test-only source directory outside the package, and no way to test a private
function directly.

The language side of this — the `test` declaration, `core/testing/assert`, and
the `context` form — is [`SPEC.md` §11.2 and §11.3](../SPEC.md). This document
is the build system's half: where tests live, what they may import, and how they
run.

```
lib/money/
  BUILD.buri
  lib.buri
  cents.buri
  parse.buri
  test/
    cents.buri
    parse.buri
```

```textproto schema=build
library {
  sources: ["cents.buri", "parse.buri"]

  test {
    sources: [
      "test/cents.buri",
      "test/parse.buri",
    ]
  }
}
```

A module listed in a `test.sources` is a **test source**. That is the only thing
that makes one: `test` declarations and imports of test-only modules are legal
there and nowhere else, and the file is compiled into a test binary rather than
into the library.

## A test

```buri repo=cli/tests/example role=test
// lib/money/test/cents.buri

from "//lib/money/lib.buri" import { fromCents, fromDollars };
from "core/testing/assert/lib.buri" import * as assert;
from "core/testing/context/lib.buri" import { Hermetic };

test "pads the cents place" {
  let ctx = Hermetic();
  assert.eq(fromCents(1905).format(ctx), "\$19.05");
}

test "addition composes" {
  assert.eq(fromDollars(19).add(fromCents(499)), fromCents(2399));
}
```

A test takes no parameters and returns nothing. It passes unless an assertion in
it fails, and a failing assertion ends that test and no other.

A title is used once per file. Two tests in one module with the same title are a
compile error (`duplicate-test-name`), because a title is how a failure is
reported and how `--filter` picks one out, and two that share it in one file
cannot be told apart. Two files of one suite may share a title: they are
separate modules, each failure names its own file, and each is reported at its
own line.

A test that needs a context builds one, with the same `context` form `main` uses
— a test source and `main`'s body are the only places in the language where a
context is created rather than received, which is why `core/testing/context` is
importable only from a test source. A pure assertion, like the second one above,
needs no context at all, and the fact that you can see which is which from the
test body is the point of the effect system showing up here too.

`core/testing/assert` is an ordinary module — the name `assert` comes from
`import * as assert`, and nothing stops a file calling it something else.
`assert.eq`, `assert.notEq`, `assert.isTrue`, `assert.isFalse`, and
`assert.fail` return `()`, so they stand alone as statements, which is the one
place the language admits an expression statement. What the rule asks is the
type, not the shape: any expression of type `()` may stand alone — a `match`
whose arms all assert, an `if`, a block — and each is terminated by `;`, the
same as a call. `assert.ok`, `assert.err`, and `assert.some` return the
unwrapped value, which is how a `Result` is consumed in a test:

```buri repo=cli/tests/example role=test
# from "//lib/money/lib.buri" import { parse, ParseError };
# from "core/testing/assert/lib.buri" import * as assert;
test "rejects text that is not a number" {
  let e = assert.err(parse("nineteen"));
  assert.eq(e, ParseError.NotANumber { text: "nineteen" });
}
```

A `match` that asserts differently per variant is a statement in exactly the
same way, and the `;` after its `}` is what says so — leave it off and the
`match` reads as the test body's result, which is what the block would have
returned had anything followed it:

```buri repo=cli/tests/example role=test
# from "//lib/money/lib.buri" import { parse, ParseError };
# from "core/testing/assert/lib.buri" import * as assert;
test "either outcome is asserted where it lands" {
  match (parse("19.99")) {
    .Ok(_) => assert.isTrue(true),
    .Err(_) => assert.fail("19.99 is a number"),
  };
  assert.isFalse(false);
}
```

`Result` is still must-use here, so an assertion you forget to check is not
something a test source can contain: there is no statement form that drops one.

## What a test can reach

A test source may import:

| | |
|---|---|
| The target under test | `//lib/money/lib.buri` for a library, `//cmd/server/main.buri` for a binary |
| The target's `dependencies` | The same libraries the target itself depends on |
| The suite's `test.dependencies` | Fakes, fixtures, matchers |
| `core/*` | Including the test platform: `core/testing/assert/lib.buri`, `core/host/testing/lib.buri`, `core/testing/context/lib.buri` |

| Any test-only path | `//lib/ledger/testing/lib.buri`, `//lib/testing/fakes/lib.buri` — the package is declared in `test.dependencies` like any other library |

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

## Shared fixtures and fakes

A helper that more than one suite needs is not a test source — it is ordinary
library code that happens to be test-only, and it lives behind a path with a
`testing` segment ([`libraries.md`](./libraries.md#the-testing-surface)):

```buri repo=cli/tests/example package=//lib/ledger role=testing
# from "core/testing/context/lib.buri" import { Hermetic, files };
# from "core/effect/lib.buri" import { Fs };
// lib/ledger/testing/fixtures.buri — inside //lib/ledger, so it can use the
// library's internals to build a fixture.

from "//lib/ledger/entry.buri" import { Entry, entry };
from "//lib/money/lib.buri" import { fromCents, fromDollars };

/// A three-entry ledger, one of them zero, for anyone testing against ledgers.
export fn sample(): [Entry] {
  [
    entry("coffee", fromCents(450)),
    entry("refund", fromCents(0)),
    entry("books", fromDollars(32)),
  ]
}

/// A context whose filesystem already holds a ledger, for suites that would
/// otherwise write the same three lines. A `context` declaration may be
/// exported only from a path with a `testing` segment ([`SPEC.md` §11.3]).
export context WithLedger {
  ..Hermetic(),
  Fs: files([("ledger.log", "coffee\t\$4.50\n")]),
}
```

A `testing { sources: [...] }` block in `lib/ledger/BUILD.buri` is what puts
that file in the build. A consumer's suite then reaches it the way it reaches
any library — declared, and by label:

```buri repo=cli/tests/example package=//tools/report role=test
// tools/report/test/render.buri — a different package's suite, using it. A
// name reaches this suite only if `testing/lib.buri` re-exports it, exactly as
// `lib.buri` decides the library's own surface one level up.
from "//lib/ledger/testing/lib.buri" import { sample, oneOff };
```

with `test { dependencies: ["//lib/ledger/testing"] }` in that package's rule.

Prefer this to a private helper as soon as a second suite wants the same
fixture, and prefer a private helper while only one does — a fixture on a
public surface is an API, and it will be depended on.

## Testing a binary

A binary's `main.buri` is its surface, exactly as `lib.buri` is a library's, and
its test sources import it by its module path:

```buri repo=cli/tests/example package=//cmd/server
// cmd/server/main.buri

from "//cmd/server/routes.buri" export { Route, route };

from "//cmd/server/routes.buri" import { route };
from "core/effect/lib.buri" import { Alloc, Stdout };
from "core/host/lib.buri" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
  };
  let _ = ctx.println("listening on ${route("/entries").name}");
  .Ok(())
}
```

```buri repo=cli/tests/example package=//cmd/server role=test
# from "core/testing/assert/lib.buri" import * as assert;
// cmd/server/test/routes.buri

from "//cmd/server/main.buri" import { route };

test "unknown paths route to the fallback" {
  assert.eq(route("/nope").name, "fallback");
}
```

`//cmd/server/main.buri` is a module inside the package, so only that package's
own test sources may import it — the same rule that keeps
`//lib/money/cents.buri` internal. Everything else is identical to testing a library: the test sees what
`main.buri` exports and nothing more, so pushing logic behind the entry point is
what makes it testable, which is the pressure you want.

**`main` itself is not testable**, and that is deliberate. It takes no
parameters and builds its own context out of `core/host`, so there is no fake to
hand it ([`SPEC.md` §11](../SPEC.md)). A binary whose failure modes you want to
assert on puts them in a function that takes an ordinary bounded `ctx`:

```buri
# from "core/effect/lib.buri" import { Alloc, Env, Fs, Stdout };
# from "core/host/lib.buri" import * as host;
# from "core/env/lib.buri" import * as env;
# from "core/fs/lib.buri" import * as fs;
// cmd/server/main.buri
export fn run<C: Alloc + Stdout + Fs>(ctx: C, path: Str): Result<(), Str> {
  fs.writeText(ctx, path, "started\n").mapErr(fn(e) => "could not write the ledger log")
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
    Fs:     host.fs,
    Env:    host.env,
  };
  run(ctx, env.get(ctx, "LEDGER_LOG") ?? "ledger.log")
}
```

`main`'s context has to bind `Env` for `env.get` as much as it binds `Fs` for
`run` — the bounds a body reaches for are the bounds the entry point has to
supply, and it is the same list in both directions.

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
# from "core/testing/context/lib.buri" import { Hermetic, data, readOnly };
# from "core/effect/lib.buri" import { Alloc, Fs, Stdout };
# from "core/fs/lib.buri" import * as fs;
# fn run<C: Alloc + Stdout + Fs>(ctx: C, path: Str): Result<(), Str> {
#   fs.writeText(ctx, path, "started\n").mapErr(fn(e) => "could not write the ledger log")
# }
// cmd/server/test/run.buri
test "run fails cleanly when the log is unwritable" {
  let ctx = context { ..Hermetic(), Fs: readOnly(data()) };
  let msg = assert.err(run(ctx, "ledger.log"));
  assert.isTrue(msg.contains("ledger"));
}
```

## The runner's context

`core/testing/context` exports one implementation per effect, not one
pre-assembled world. Each is real where it can be and hermetic everywhere else:

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Alloc` | Real, with a per-test arena the runner reclaims. |
| `captureOut()`, `captureErr()` | `Stdout`, `Stderr` | Captured, and never printed; `captured()` is how a test reads it back. |
| `stdin([Str])` | `Stdin` | Reads the given lines, then end-of-input. |
| `data()` | `Fs` | In-memory, rooted at the package directory, containing exactly `test.data`. Writes are visible to that test and discarded after it. |
| `files([(Str, Str)])` | `Fs` | In-memory, containing exactly these entries. |
| `readOnly(F)` | `Fs` | Wraps an `Fs` so every write fails. |
| `noNet()` | `Net` | Refuses every connection. A fake goes in `test.dependencies`. |
| `clockAt(Int)` | `Clock` | Starts there and advances only when the test advances it. |
| `randSeed(Int)` | `Rand` | Seeded, so a failure reproduces. |
| `envOf([(Str, Str)], [Str])` | `Env` | These variables and these arguments. |

`Hermetic` is a context binding all of them at their defaults — an empty
`envOf`, `clockAt(0)`, `randSeed(0)`, and `data()` for the filesystem. A file
may use it directly, declare its own on top of it, or build one per test:

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
# from "core/testing/context/lib.buri" import { Hermetic, envOf };
# from "core/effect/lib.buri" import { Env };
# from "core/env/lib.buri" import * as env;
# fn logPath<C: Env>(ctx: C): Str { env.get(ctx, "LEDGER_LOG") ?? "ledger.log" }
context Fixture {
  ..Hermetic(),
  Env: envOf([("LEDGER_LOG", "custom.log")], ["--verbose"]),
}

test "reads the log path from the environment" {
  let ctx = Fixture();
  assert.eq(logPath(ctx), "custom.log");
}

test "falls back when the variable is unset" {
  let ctx = context { ..Fixture(), Env: envOf([], []) };
  assert.eq(logPath(ctx), "ledger.log");
}
```

Each call builds a fresh context, so what one test writes to its filesystem or
prints to its captured stdout is invisible to the next — which is why a named
context is called rather than referred to.

### `core/host/testing` — the same doubles, under `core/host`'s names

`core/host/testing` is the platform a test source binds, and it is
`core/host`'s surface written out for a test: the same names — `alloc`,
`stdout`, `stderr`, `stdin`, `fs`, `net`, `clock`, `rand`, `env`, `proc` —
**called** rather than referred to. `core/host`'s `clock` is one clock because a process
has one; `clock()` is a fresh clock every call, so a test never inherits another
test's.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Alloc` | Real, with a per-test arena the runner reclaims. |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | Captured, and never printed; `captured()` reads either one back. |
| `stdin()` | `Stdin` | At end of input, so a suite never blocks on a pipe nobody is writing to. |
| `fs()` | `Fs` | In-memory and empty. Writes are visible to that test and discarded after it. |
| `net()` | `Net` | **Refuses** every request with `.Refused`, until `respond` says what to answer. |
| `clock()` | `Clock` | At zero. `sleepMillis` advances it without sleeping. |
| `rand()` | `Rand` | Seeded at zero, so a failure reproduces. |
| `env()` | `Env` | No variables and no arguments. |
| `proc()` | `Proc` | **Absorbs** the exit instead of taking it, so the test carries on. |

Configuration is a **method on the value that answers a new handle**, so a
chain reads in the order it is applied and the value it was called on is
unchanged:

| Builder | Answers |
|---|---|
| `clock().at(1000)` | A clock at that instant |
| `rand().seed(7)` | A generator at that seed, from the start of its sequence |
| `env().variables([(Str, Str)])` | An environment with those variables and this one's arguments |
| `env().args([Str])` | An environment with those arguments and this one's variables |
| `stdin().lines([Str])` | A stream of those lines, then end of input |
| `stdin().bytes([U8])` | A stream of those octets, then end of input |
| `fs().files([(Str, Str)])` | A filesystem holding this one's files and these as well |
| `fs().filesBytes([(Str, [U8])])` | The byte twin, for a fixture that is not text |
| `fs().readOnly()` | The **same** files, through a handle whose every write fails with `.ReadOnly` |
| `net().respond(fn(Request) => Result<Response, NetError>)` | A network answering every request through that function |

`args` and not `arguments`, and it is the one name here that is not
`core/host`'s: `Env` already declares `arguments(self): [Str]` — the reader —
and a type has one method of each name. A type's methods are one map keyed by
name, and a method written in `impl Env for TestEnv` goes into it beside the
ones written in `impl TestEnv`, so neither the extra argument nor the different
return type tells the two apart — the builder would simply be refused, and a
call to it read as the reader with an argument too many. If the builder is ever
to be spelled `arguments`, the *reader* is what moves.

`lines` and `bytes` are the one pair that **replace** each other rather than
composing: a stream is either the lines a test wrote or the octets it wrote, a
stdin built from octets answers `.None` to `readLine`, and the last builder in
the chain is the stream. `files` and `filesBytes` do compose, in either order,
because both write into the one map a file lives in.

`readOnly()` is `core/testing/context`'s `ReadOnly<C>` wrapper folded into a
method, and the fold keeps what made it a wrapper: it attenuates the *same*
filesystem rather than a copy, so a read through the attenuated handle answers
whatever the filesystem holds now. `ReadOnly<C>` itself stays, because it
attenuates any `Fs` — including one a test wrote — and the method attenuates
only this one.

### Reading the environment back

The outcome of a test is the return value **plus the environment read back**.
`captured()` does that for a stream, and `TestFs` has two of its own:

| Read-back | Answers |
|---|---|
| `read(path)` | `Result<Str, IoError>` — what the filesystem holds there, the same answer `readFile` gives |
| `snapshot()` | `[(Str, Str)]` — every file, as text, **sorted by path** |
| `calls()` | `[FsCall]` — every call made through this handle, **in the order they completed** |

Neither needs the `Fs` effect bound: asserting on what a function wrote is
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

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
from "core/host/testing/lib.buri" import { alloc, fs };
from "core/effect/lib.buri" import { Alloc, Fs, IoError };
# fn archive<C: Fs>(ctx: C, path: Str): Result<(), IoError> {
#   match (ctx.readFile(path)) {
#     .Err(e) => .Err(e),
#     .Ok(body) => ctx.writeFile("{path}.bak", body),
#   }
# }

test "archiving leaves the original alone and writes the copy beside it" {
  let files = fs().files([("notes.txt", "hello")]);
  let ctx = context { Alloc: alloc(), Fs: files };
  assert.ok(archive(ctx, "notes.txt"));
  assert.eq(files.snapshot(), [("notes.txt", "hello"), ("notes.txt.bak", "hello")]);
}

test "a read-only filesystem refuses the write, and nothing is written" {
  let files = fs().files([("notes.txt", "hello")]);
  let ctx = context { Alloc: alloc(), Fs: files.readOnly() };
  assert.eq(assert.err(archive(ctx, "notes.txt")), .ReadOnly);
  assert.eq(files.snapshot(), [("notes.txt", "hello")]);
}
```

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
from "core/host/testing/lib.buri" import { alloc, clock, env, stdout };
from "core/effect/lib.buri" import { Alloc, Clock, Env, Stdout };
# fn logPath<C: Env>(ctx: C): Str { ctx.variable("LEDGER_LOG") ?? "ledger.log" }

test "reads the log path from the environment" {
  let ctx = context {
    Alloc: alloc(),
    Env: env().variables([("LEDGER_LOG", "custom.log")]).args(["--verbose"]),
  };
  assert.eq(logPath(ctx), "custom.log");
}

test "a context names only the effects the function under test needs" {
  let sink = stdout();
  let ctx = context { Alloc: alloc(), Clock: clock().at(1000), Stdout: sink };
  let now = ctx.nowMillis();
  let _ = ctx.println("started at {now}");
  assert.eq(sink.captured(), "started at 1000\n");
}
```

The second block is the shape to copy: a test context binds what the function
needs and nothing else, rather than a pre-assembled world. `core/testing/context`
and its `Hermetic()` keep working unchanged while the corpus moves across.

### A network that answers

`net()` refuses everything, and that is the default worth having: a test that
reaches the network by accident says so at its assertion rather than passing on
an answer nobody wrote. `respond` hands it a function, and that function is the
fake server — it is given every `Request` the code under test makes, and either
answers it or fails it.

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
from "core/host/testing/lib.buri" import { alloc, net };
from "core/effect/lib.buri" import { Alloc, Net, NetError, Request };
from "core/net/http/lib.buri" import * as http;
# fn load<C: Net>(ctx: C, request: Request): Result<Int, NetError> {
#   http.send(ctx, request).map(fn(r) => r.status)
# }

test "a request nobody arranged for is refused rather than answered" {
  let ctx = context { Alloc: alloc(), Net: net() };
  let asked = load(ctx, http.request(.Get, "https://example.test/a"));
  assert.eq(assert.err(asked), NetError.Refused);
}

test "the responder decides on the method and on a header" {
  let ctx = context { Alloc: alloc() };
  let page = http.text(ctx, "Ledger");
  let server = net().respond(fn(request) => {
    let authorized = request.header("authorization") == .Some("Bearer t0ken");
    match ((request.method, authorized)) {
      (.Get, true) => .Ok(page),
      (_method, true) => .Ok(http.status(405)),
      (_any, false) => .Ok(http.status(401)),
    }
  });
  let live = context { Alloc: alloc(), Net: server };
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
([`SPEC.md` §10.6](../SPEC.md)), so a responder cannot call `http.text(ctx, ...)`
inside itself — which is precisely what makes a
`fn(Request) => Result<Response, NetError>` a pure function of the request and
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
effects are ordinary interfaces ([`SPEC.md` §10.9](../SPEC.md)), and it is bound
exactly the way the runner's own implementations are:

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
# from "core/testing/context/lib.buri" import { Hermetic };
# from "core/effect/lib.buri" import { Net, NetError, Request, Response };
# from "core/net/http/lib.buri" import * as http;
# fn status<C: Net>(ctx: C, url: Str): Result<Int, NetError> {
#   http.send(ctx, http.request(.Get, url)).map(fn(r) => r.status)
# }
struct StubNet { export failing: Str }

impl Net for StubNet {
  fn fetch(self, request: Request): Result<Response, NetError> {
    if (request.url == self.failing) {
      .Err(.Timeout)
    } else {
      .Ok(http.status(200))
    }
  }
}

test "a timeout reaches the caller as an error" {
  let ctx = context { ..Hermetic(), Net: StubNet { failing: "https://example.test/slow" } };
  assert.eq(assert.err(status(ctx, "https://example.test/slow")), NetError.Timeout);
  assert.eq(assert.ok(status(ctx, "https://example.test/x")), 200);
}
```

That is the general mechanism, and for `Net` in a test source `net().respond`
is the same thing already written: `StubNet.fetch` and a responder are the same
function of the same request.

The fake answers from its fields rather than from a counter, because there is no
mutation to hold one in. `clockAt`'s advancing clock and `captureOut`'s
accumulating buffer do change between calls, and that is a privilege of the
runner's own implementations rather than a mechanism a fake can borrow: each of
those constructors is an intrinsic that installs a slot in a table the runtime
owns, and nothing in `core/testing/context` hands one out. A fake you write is
an immutable struct and stays one, so it answers the same way every time it is
asked the same question.

### What the code under test asked for

`snapshot()` says what the world *is*; `calls()` says what it was **asked**.
`TestFs`, `TestNet` and `TestStdin` each keep every call made through the handle
and answer them in the order they completed:

| Log | Answers |
|---|---|
| `fs().calls()` | `[FsCall]` — one per call to any of the eleven methods of `Fs` |
| `net().calls()` | `[NetCall]` — one per request, whole: method, URL, headers and body |
| `stdin().calls()` | `[StdinCall]` — one per `readLine` or `readBytes`, with what it asked for |

A test writes the call it expects with the constructor of the same name, and
these are ordinary functions of `core/host/testing`: `readFile(path)`,
`writeFile(path, body)`, `renameFile(source, destination)`, `fetch(request)`,
`readBytes(n)` — one per method, taking the call's own arguments. They derive
`Eq`, which is what an assertion compares, and `Show`, which is what a failing
one prints.

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
from "core/host/testing/lib.buri" import { alloc, fs, net, readFile, fetch };
from "core/effect/lib.buri" import { Alloc, Fs, IoError, Net, NetError, Response };
from "core/net/http/lib.buri" import * as http;
# fn cached<C: Alloc + Fs + Net>(ctx: C, url: Str): Result<Response, NetError> {
#   match (ctx.readFile("cache")) {
#     .Ok(_body) => .Ok(http.status(200)),
#     .Err(_e) => http.get(ctx, url),
#   }
# }

test "a miss consults the cache once and then goes upstream" {
  let files = fs();
  let upstream = net().respond(fn(_request) => .Ok(http.status(200)));
  let ctx = context { Alloc: alloc(), Fs: files, Net: upstream };
  let _ = assert.ok(cached(ctx, "https://example.test/thing"));
  assert.eq(files.calls(), [readFile("cache")]);
  assert.eq(upstream.calls(), [fetch(http.request(.Get, "https://example.test/thing"))]);
}

test "a hit never reaches the network at all" {
  let files = fs().files([("cache", "hit")]);
  let upstream = net();
  let ctx = context { Alloc: alloc(), Fs: files, Net: upstream };
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

### Making the Nth call fail

That boundary is where deterministic simulation meets it. "The third write
fails" wants a counter, and a counter is the state a fake cannot hold — so the
fault is expressed as a value the test chooses rather than a moment the fake has
to recognise.

Split each durable operation into three: a pure `prepare` that decides what to
write, one effectful `persist` that writes it, and a pure `publish` that folds
the outcome back into the state. `prepare` and `publish` take no context, so a
test calls them directly and reads what they answer; `persist` is the only step
that reaches the `Fs`, and it is called once per step, so a test that wants the
third one to fail runs the first two and hands the third an `.Err` of its own.
The recovery path — the state after a write that did not happen — is then
reached with ordinary values through the real code, and the crash between a
log append and the sequence advance that follows it is a step boundary rather
than something to interrupt mid-call.

What this does not cover is a failure *inside* one effectful step: if `persist`
makes three `fs.writeText` calls, a test can fail all three or none of them.
Keeping `persist` to a single call is what makes that distinction go away, and
it is worth the split on its own — a step that writes once is a step whose
failure has one meaning.

This is defence in depth rather than the primary mechanism. The primary
mechanism is that a test whose call never passed a `Net`-bounded context cannot
open a socket in anything it transitively calls — that is
[`SPEC.md` §10](../SPEC.md), not a build system feature. There is no third layer:
the toolchain applies no operating-system confinement, because a suite has no
name for a real capability to begin with
([`hermeticity.md`](./hermeticity.md)).

## Test data and golden files

`test { data: [...] }` declares the files the in-memory `Fs` contains:

```textproto ignore why="a fragment of a build file, not a whole one"
test {
  sources: ["test/ledger.buri"]
  data: ["test/golden/statement.txt"]
  timeout_seconds: 30
}
```

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
# from "core/testing/context/lib.buri" import { Hermetic };
# from "core/effect/lib.buri" import { Alloc, Fs };
# from "core/fs/lib.buri" import * as fs;
# struct Entry { export memo: Str }
# fn sample(): [Entry] { [Entry { memo: "coffee" }] }
# fn render<C: Alloc>(ctx: C, entries: [Entry]): Str {
#   entries.mapCtx(ctx, fn(c, e) => e.memo).join(ctx, "\n")
# }
test "renders the statement" {
  let ctx = Hermetic();          // its `Fs` is `data()`, so the golden file is there
  let want = assert.ok(fs.readText(ctx, "test/golden/statement.txt"));
  assert.eq(render(ctx, sample()), want);
}
```

Rewriting a golden file is not something a hermetic action may do, so
`buri test --accept` is a separate mode: it runs the suites outside the cache,
collects the actual value from every `assert.eq` whose expected side came from a
declared `data` file, and writes those files in the source tree. It never
creates a file, never touches one not listed in `data`, prints a diff for each,
and leaves everything else about the run unchanged. The normal
`buri test` path stays hermetic and cacheable, which is what makes it safe to
have an update mode at all — the two never share a code path that writes.

## Running

```
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  substring match on test names
buri test //... --output=js          send the suites that name no platform to JS
buri test //lib/money --accept       update declared golden files
```

A suite runs as a native binary for the host unless something sends it to
JavaScript: its own `test { platforms }`, `--output=js`, `--accept`, or the
fallback for a toolchain that cannot build one (`buri docs cli test`). The
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
