---
name: buri-testing
description: Use when writing, running, or debugging Buri tests — the test declaration, assertions, contexts and fakes, golden files, and what buri test reports.
---

# Buri: writing and running tests

Tests live inside the target they test, are declared in its build rule, and can
reach only what a dependent could reach. There is no separate test target and
no way to test a private function directly. `buri docs build/testing` and
`buri docs cli test` are the normative pages.

## Declaring a suite

```
lib/money/
  BUILD.buri
  lib.buri
  cents.buri
  test/cents.buri
```

```textproto
library {
    sources: ["cents.buri"]

    test {
        sources: [
            "test/cents.buri",
            "test/parse.buri",
        ]
        dependencies: ["//lib/ledger/testing"]
        timeout_seconds: 30
        platforms: [LINUX, JS]
    }
}
```

**A module listed in `test.sources` is a test source.** That is the only thing
that makes one: `test` declarations and imports of test-only modules are legal
there and nowhere else. `buri gen` maintains `test.sources` for you.

## A test

```buri
from "//lib/money" import { fromCents, fromDollars };
from "core/testing/assert" import * as assert;
from "core/host/testing" import { alloc };
from "core/effect" import { Alloc };

test "pads the cents place" {
    let ctx = context { Alloc: alloc() };
    assert.eq(fromCents(1905).format(ctx), "\$19.05");
}

test "addition composes" {
    assert.eq(fromDollars(19).add(fromCents(499)), fromCents(2399));
}
```

- `test STRING Block`. The name is a string literal because test names are
  prose. A test takes no parameters and returns nothing; it passes unless an
  assertion in it fails, and a failing assertion ends that test and no other.
- **A title is used once per file** (`duplicate-test-name`). Two files may
  share a title.
- A pure assertion needs no context at all — and being able to see which is
  which from the body is the point.
- `assert` is not a keyword. The name comes from `import * as assert`.

### Assertions

| Function | Meaning |
|---|---|
| `assert.eq(a, b)` | fails unless `a == b`; needs `Eq`, and `Show` for the message |
| `assert.notEq(a, b)` | the negation |
| `assert.isTrue(b)` / `assert.isFalse(b)` | on a `Bool` |
| `assert.fail(msg)` | fails unconditionally |
| `assert.ok(r)` | fails unless `r` is `.Ok`; **returns the wrapped value** |
| `assert.err(r)` | fails unless `r` is `.Err`; returns the error |
| `assert.some(o)` | fails unless `o` is `.Some`; returns the wrapped value |

The first five return `()`, so they stand alone as statements — a test source is
the one place the language admits an expression statement, and only when the type
is `()`. Any expression of that type qualifies, not only a call: a `match` whose
arms all assert is a statement too, terminated by `;` like the rest. The last
three return a value, and are how a `Result` is consumed in a test, since
`Result` is still must-use here.

```buri
test "reads the config it wrote" {
    let ctx = context { Alloc: alloc(), Fs: memory() };
    assert.ok(fs.writeText(ctx, "cfg", "port=8080"));   // returns (), so a statement
    let text = assert.ok(fs.readText(ctx, "cfg"));      // returns Str, so a binding
    assert.eq(text, "port=8080");
}
```

If `assert.eq` reports `unsatisfied-bound`, the type under test is missing
`derive Eq, Show for ThatType;` in **its own** module.

## The runner's context

`core/host/testing` is `core/host`'s surface written out for a test: the same
names, **called** rather than referred to, so each call answers a fresh double.
One per effect, not a pre-assembled world. Importable only by a test source.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Alloc` | real, from a per-test arena the runner reclaims |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | captured and never printed; `captured()` reads either back |
| `stdin()` | `Stdin` | at end of input, so a suite never blocks on a pipe nobody writes to |
| `fs()` | `Fs` | in-memory and empty; writes are discarded after the test |
| `net()` | `Net` | refuses every request until `respond` says what to answer |
| `clock()` | `Clock` | at zero; `sleepMillis` advances it without sleeping |
| `rand()` | `Rand` | seeded at zero, so a failure reproduces |
| `env()` | `Env` | no variables and no arguments |
| `proc()` | `Proc` | absorbs the exit instead of taking it, so the test carries on |
| `tasks()` | `Tasks` | runs the tasks one at a time, in program order |

Configuration is a **method answering a new handle**, leaving the value it was
called on alone: `clock().at(n)`, `rand().seed(n)`,
`env().variables([...]).arguments([...])`, `stdin().lines(...)` or `.bytes(...)`
(these replace), `fs().files(...)` and `.filesBytes(...)` (these compose),
`fs().readOnly()`, `net().respond(fn(Request) => ...)`, `tasks().anyOrder()`.
Read the environment back with `captured()`, `fs().read(p)`, `fs().snapshot()`
and `calls()` — what the code under test **asked** for; `faults([...])` says what
fails, and a fault whose call never happens fails the test.

```buri
context Fixture {
    Alloc: alloc(),
    Env: env().variables([("LEDGER_LOG", "custom.log")]).arguments(["--verbose"]),
}

test "reads the log path from the environment" {
    let ctx = Fixture();
    assert.eq(logPath(ctx), "custom.log");
}

test "falls back when the variable is unset" {
    let ctx = context { ..Fixture(), Env: env() };
    assert.eq(logPath(ctx), "ledger.log");
}
```

**Each call builds a fresh context**, which is why a named context is called
rather than referred to: what one test writes to its filesystem or its captured
stdout is invisible to the next. Bind what the function needs and nothing else,
and reach a double the way the real thing is reached: `io.println(ctx, "x")`.

## Fakes

A test double is an ordinary struct with methods, because effects are ordinary
interfaces. There is no mocking framework and no global to stub.

```buri
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
    let ctx = context { Alloc: alloc(), Net: StubNet { failing: "https://example.test/slow" } };
    assert.eq(assert.err(status(ctx, "https://example.test/slow")), NetError.Timeout);
}
```

A fake answers from its fields rather than from a counter — there is no
mutation to hold one in. Keeping state between calls is the runner's own
privilege: `clock()` and `stdout()` are intrinsics holding a slot in a table
the runtime owns, and a fake you write cannot get one. "The third write fails"
is a fault plan (`fs().faults([...])`); a crash *between* two calls is a step
boundary — split it into a pure `prepare`, one effectful `persist` and a pure
`publish`, and hand the step you choose an `.Err`.

Defence in depth: a suite whose calls never passed a `Net`-bounded context
cannot open a socket at all, so no operating-system confinement is applied.

## What a test source may and may not do

May import: the target under test (`//lib/money`, or
`//cmd/server/main.buri` for a binary), the target's `dependencies`, the suite's
`test.dependencies`, `core/*` including the test platform, and any test-only path.

May **not**: import a library-internal module (`//lib/money/cents.buri` →
`test-internal-import`); import another test source (they are compiled
independently); be imported by anything; `export` anything.

If a test needs an internal function, either the function belongs on the surface
— say so in `lib.buri` — or the test asserts on an implementation detail.

**`main` itself is not testable.** It takes no parameters and builds its own
context out of `core/host`, so there is no fake to hand it. Put the logic in a
function taking an ordinary bounded `ctx`:

```buri
export fn run<C: Alloc + Stdout + Fs>(ctx: C, path: Str): Result<(), Str> {
    fs.writeText(ctx, path, "started\n").mapErr(fn(e) => "could not write the ledger log")
}
```

```buri
test "run fails cleanly when the log is unwritable" {
    let ctx = context { Alloc: alloc(), Stdout: stdout(), Fs: memory().readOnly() };
    let msg = assert.err(run(ctx, "ledger.log"));
    assert.isTrue(msg.contains("ledger"));
}
```

## Shared fixtures

A helper more than one suite needs is not a test source — it is ordinary library
code behind a path with a `testing` segment, declared by a
`testing { sources: [...] }` block. It may import the library's internals, has
its own `dependencies`, is never linked into a production artifact, and inherits
the library's `visibility` and `tags`. A consumer's suite reaches it by label,
declared in `test { dependencies }`.

Prefer a private helper while only one suite wants the fixture, and promote it
as soon as a second does — a fixture on a public surface is an API.

## Golden files

Write a suite's filesystem in the suite, with `core/host/testing`'s `fs().files`:

```buri
from "core/host/testing" import { alloc, fs as memory };

test "renders the statement" {
    let ctx = context { Alloc: alloc(), Fs: memory().files([("statement.txt", "coffee")]) };
    let want = assert.ok(fs.readText(ctx, "statement.txt"));
    assert.eq(render(ctx, sample()), want);
}
```

A golden read straight back out of that filesystem is usually shorter as a value
in the assertion. The filesystem earns its place when the code under test reads.

Both `test { data: [...] }` and `buri test --accept` are retired: the field made
a suite's filesystem a fact about the build that only the JavaScript runner could
supply — a linked test binary has no runner — so the backends disagreed. A golden
is a value in the suite's own source now, and no backend is refused one.

## Running

```
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  substring match on test names
buri test //... --output=js          send the suites that name no platform to JS
buri test //... --watch              re-run on every change to a declared input
buri test //... --explain            one line per action: ran, or served by the cache
```

Exit status is `0` when every test passed and `1` when any did not, so
`buri test` is usable directly as a gate.

```
FAIL //lib/money  test/cents.buri  "pads the cents place"
  assert.eq failed
    actual:   "$19.5"
    expected: "$19.05"
  --> lib/money/test/cents.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

A suite that never compiled has no cases, so it is counted separately and only
when there is one: `0 passed, 0 failed, 0 skipped, 1 failed to compile (0.0s)`.

Tests are ordinary build actions: a suite whose sources, target, dependencies
and toolchain are unchanged is not re-run and reports as **cached**. Because
there is no mutable global state and no observable ordering, the runner may
shard and reorder freely, and there is no flag to turn that off.

A suite runs natively on the host. Two things send it to JavaScript, and both
are somebody saying so: `test { platforms: [JS] }`, or `--output=js`. Nothing
else does — a program the backend has no body for, or a toolchain that cannot
build for this host, is an **error** (`native-run-not-available`, or
`platform-not-implemented` where the suite named the platform), never a reroute.

Suites that name no platform are compiled into one binary per tag-compatible
batch and linked once. Verdicts, caching and reports are still per suite; a
`test { platforms }`, `timeout_seconds` or `--output=` keeps a suite out of a
batch.

## Lint findings about tests

`empty-test-suite` (a `test` block with no `sources`),
`test-without-assertion` (nothing reachable from the test calls into
`core/testing/assert` — transitive, so asserting through a helper is fine),
`test-title-newline`, and at run time `test-timeout`,
`platform-not-implemented` and `native-run-not-available`.
