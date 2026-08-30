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
| The target under test | `//lib/money` for a library, `//lib/money/main` for a binary |
| The target's `dependencies` | The same libraries the target itself depends on |
| The suite's `test.dependencies` | Fakes, fixtures, matchers |
| `core/*` | Including the test platform: `core/testing/assert`, `core/testing/context` |

| Any test-only path | `//lib/ledger/testing`, `//lib/testing/fakes` — declared in `test.dependencies` like any other library |

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
   |      ^^^^^^^^^^^^^^^^^^^
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

`//cmd/server/main` is a module inside the package, so only that package's own
test sources may import it — the same rule that keeps `//lib/money/cents`
internal. Everything else is identical to testing a library: the test sees what
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

Anything the runner does not provide is an ordinary struct with methods, since
effects are ordinary interfaces ([`SPEC.md` §10.9](../SPEC.md)), and it is bound
exactly the way the runner's own implementations are:

```buri role=test
# from "core/testing/assert/lib.buri" import * as assert;
# from "core/testing/context/lib.buri" import { Hermetic };
# from "core/effect/lib.buri" import { Net, NetError, NetResponse };
# fn body<C: Net>(ctx: C, url: Str): Result<Str, NetError> {
#   ctx.fetch("GET", url, "").map(fn(r) => r.body)
# }
struct StubNet { export failing: Str }

impl Net for StubNet {
  fn fetch(self, method: Str, url: Str, body: Str): Result<NetResponse, NetError> {
    if (url == self.failing) {
      .Err(.Timeout)
    } else {
      .Ok(NetResponse { status: 200, body: "{}" })
    }
  }
}

test "a timeout reaches the caller as an error" {
  let ctx = context { ..Hermetic(), Net: StubNet { failing: "https://example.test/slow" } };
  assert.eq(assert.err(body(ctx, "https://example.test/slow")), NetError.Timeout);
  assert.eq(assert.ok(body(ctx, "https://example.test/x")), "{}");
}
```

The fake answers from its fields rather than from a counter, because there is no
mutation to hold one in. `clockAt`'s advancing clock and `captureOut`'s
accumulating buffer do change between calls, and that is a privilege of the
runner's own implementations rather than a mechanism a fake can borrow: each of
those constructors is an intrinsic that installs a slot in a table the runtime
owns, and nothing in `core/testing/context` hands one out. A fake you write is
an immutable struct and stays one, so it answers the same way every time it is
asked the same question.

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
