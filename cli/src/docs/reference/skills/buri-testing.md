---
name: buri-testing
description: Use when writing, running, or debugging Buri tests — the test declaration, assertions, contexts and fakes, golden files, and what buri test reports.
---

# Buri: writing and running tests

Tests live inside the target they test. Its build rule declares them, and they
reach only what a dependent could reach. There is no separate test target, and
no way to test a private function directly. `buri docs build/testing` and `buri
docs cli test` are the normative pages.

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

**A module listed in `test.sources` is a test source**, and nothing else makes
one. `test` declarations and imports of test-only modules are legal there and
nowhere else. `buri gen` maintains `test.sources` for you.

## A test

```buri
from "//lib/money" import { fromCents, fromDollars };
from "core/testing/assert" import * as assert;
from "core/host/testing" import { alloc };
from "core/effect" import { Allocator };

test "pads the cents place" {
    let ctx = context { Allocator: alloc() };
    assert.eq(fromCents(1905).format(ctx), "\$19.05");
}

test "addition composes" {
    assert.eq(fromDollars(19).add(fromCents(499)), fromCents(2399));
}
```

- `test STRING Block`. A test takes no parameters and returns nothing. It
  passes unless an assertion in it fails, and a failing assertion ends that
  test and no other.
- **Use a title once per file** (`duplicate-test-name`); two files may share
  one. A pure assertion needs no context at all. `assert` is not a keyword: the
  name comes from `import * as assert`.

### Assertions

| Function | Meaning |
|---|---|
| `assert.eq(a, b)` / `notEq` | fails unless `a == b`; needs `Eq`, and `Show` for the message |
| `assert.isTrue(b)` / `isFalse` | on a `Bool` |
| `assert.contains(xs, x)`, `isEmpty(xs)` / `notEmpty`, `len(xs, n)` | on a list |
| `assert.gt(a, b)` / `ge` / `lt` / `le`, `approxEq(a, b, tolerance)` | on an `Ord`, and on `Float` within an absolute tolerance |
| `assert.ok(r)` | fails unless `r` is `.Ok`; **returns the wrapped value** |
| `assert.err(r)` | fails unless `r` is `.Err`; returns the error |
| `assert.some(o)` | fails unless `o` is `.Some`; returns the wrapped value |

Reach for the narrowest one that fits: each names both values in its report,
while `assert.isTrue(xs.contains(x))` says only "expected true, got false".
There is no `assert.fail`. Everything but the last three returns `()`, so they
stand alone as statements; the last three return a value, which is how you use
up a must-use `Result`. A test source is the one place the language admits an
expression statement, and only at type `()`, terminated by `;`.

```buri
test "reads the config it wrote" {
    let disk = memory();                            // one filesystem, two effects
    let ctx = context { Allocator: alloc(), FileSystemRead: disk, FileSystemWrite: disk };
    let cfg = path.of(ctx, "cfg");                  // core/fs takes a Path
    assert.ok(fs.writeText(ctx, cfg, "port=8080")); // returns (), so a statement
    let text = assert.ok(fs.readText(ctx, cfg));    // returns Str, so a binding
    assert.eq(text, "port=8080");
}
```

If `assert.eq` reports `unsatisfied-bound`, the type under test needs
`derive Eq, Show for ThatType;` in **its own** module.

## The runner's context

`core/host/testing` is `core/host`'s surface written out for a test: the same
names, **called** rather than referred to. Each call hands back a fresh double,
one per effect, and only a test source may import it.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Allocator` | real, from a per-test arena the runner reclaims |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | captured and never printed; `captured()` reads either back |
| `stdin()` | `Stdin` | at end of input, so a suite never blocks on a pipe nobody writes to |
| `fs()` | `FileSystemRead`, `FileSystemWrite` | in-memory and empty; writes discarded after the test. One call is one filesystem answering **both** effects, so a context that reads and writes binds the same value under both names |
| `net()` | `Network` | refuses every request until `respond` says what to answer |
| `clock()` | `Clock` | at zero; `sleepMilliseconds` advances it without sleeping |
| `rand()` | `Random` | seeded at zero, so a failure reproduces |
| `env()` | `Environment` | no variables and no arguments |
| `proc()` | `Process` | absorbs the exit instead of taking it, so the test carries on |
| `tasks()` | `Tasks` | runs the tasks one at a time, in program order |

You configure a double with a **method returning a new handle**, which leaves
the one you called it on alone: `clock().at(n)`, `rand().seed(n)`,
`env().variables([...]).withArguments([...])`, `stdin().lines(...)` or `.bytes(...)`
(these replace), `fs().files(...)` and `.filesBytes(...)` (these compose),
`fs().readOnly()`, `net().respond(fn(Request) => ...)`, `tasks().anyOrder()`.
Read back what happened with `captured()`, `fs().read(p)`, `fs().snapshot()` and
`calls()`, which is what the code under test **asked** for. `faults([...])` says
what fails, and a fault whose call never happens fails the test.

```buri
context Fixture {
    Allocator: alloc(),
    Environment: env().variables([("LEDGER_LOG", "custom.log")]).withArguments(["--verbose"]),
}

test "reads the log path from the environment" {
    let ctx = Fixture();
    assert.eq(logPath(ctx), "custom.log");
}

test "falls back when the variable is unset" {
    let ctx = context { ..Fixture(), Environment: env() };
    assert.eq(logPath(ctx), "ledger.log");
}
```

**Each call builds a fresh context**, so what one test writes to its filesystem
or its captured stdout is invisible to the next. One declaration cannot bind
`FileSystemRead` and `FileSystemWrite` over a single filesystem — two bindings are two `fs()`
calls, so a block that reads *and* writes names the double first. Bind what the
function needs and nothing else, and reach a double like the real thing:
`io.println(ctx, "x")`.

## Fakes

A test double is an ordinary struct with methods, because effects are ordinary
interfaces. There is no mocking framework and no global to stub.

```buri
struct StubNet { export failing: Str }

impl Network for StubNet {
    fn fetch(self, request: Request): Result<Response, NetError> {
        if (request.url == self.failing) {
            .Err(.Timeout)
        } else {
            .Ok(http.status(200))
        }
    }
}

test "a timeout reaches the caller as an error" {
    let ctx = context { Allocator: alloc(), Network: StubNet { failing: "https://example.test/slow" } };
    assert.eq(assert.err(status(ctx, "https://example.test/slow")), NetError.Timeout);
}
```

A fake answers from its fields rather than from a counter: it has no mutation
to hold one. Only the runner keeps state between calls, so "the third write
fails" is a fault plan (`fs().faults([...])`). A crash *between* two calls is a
step boundary: split it into a pure `prepare`, one effectful `persist` and a
pure `publish`, then hand the step you choose an `.Err`. A read-only fake
implements `FileSystemRead`: four methods, not twelve. A suite that never binds `Network`
cannot open a socket.

## What a test source may and may not do

May import: the target under test (`//lib/money`, or
`//cmd/server/main.buri` for a binary), the target's `dependencies`, the suite's
`test.dependencies`, `core/*` including the test platform, and any test-only path.

May **not**: import a library-internal module (`//lib/money/cents.buri` →
`test-internal-import`); import another test source, since the compiler compiles
them independently; be imported by anything; `export` anything.

If a test needs an internal function, either it belongs on the surface — say so
in `lib.buri` — or the test asserts on an implementation detail.

**You cannot test `main` itself.** It builds its own context out of `core/host`,
so you have no fake to hand it. Put the logic in a function taking an ordinary
bounded `ctx`:

```buri
export fn run<C: Allocator + Stdout + FileSystemWrite>(ctx: C, at: Path): Result<(), Str> {
    fs.writeText(ctx, at, "started\n").mapErr(fn(e) => "could not write the ledger log")
}
```

```buri
test "run fails cleanly when the log is unwritable" {
    let ctx = context { Allocator: alloc(), Stdout: stdout(), FileSystemWrite: memory().readOnly() };
    let msg = assert.err(run(ctx, path.of(ctx, "ledger.log")));
    assert.isTrue(msg.contains("ledger"));
}
```

## Shared fixtures

A helper more than one suite needs is not a test source. It is ordinary library
code behind a path with a `testing` segment, declared by a
`testing { sources: [...] }` block. It may import the library's internals,
carries its own `dependencies`, never links into a production artifact, and
inherits the library's `visibility` and `tags`. A consumer reaches it by label,
in `test { dependencies }`. A fixture on a public surface is an API.

## Golden files

Write a suite's filesystem in the suite, with `core/host/testing`'s `fs().files`:

```buri
from "core/host/testing" import { alloc, fs as memory };

test "renders the statement" {
    let ctx = context { Allocator: alloc(), FileSystemRead: memory().files([("statement.txt", "coffee")]) };
    let want = assert.ok(fs.readText(ctx, path.of(ctx, "statement.txt")));
    assert.eq(render(ctx, sample()), want);
}
```

A golden you read straight back out is usually shorter as a value in the
assertion; the filesystem earns its place when the code under test reads.

`test { data: [...] }` and `buri test --accept` are both retired: a golden lives
as a value in the suite's own source, so every backend can have one.

## Running

```
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  substring match on test names
buri test //... --output=js          send the suites that name no platform to JS
buri test //... --watch              re-run on every change to a declared input
buri test //... --explain            one line per action: ran, or served by the cache
```

`buri test` exits `0` when every test passed and `1` when any did not, so you
can use it directly as a gate.

```
FAIL //lib/money  test/cents.buri  "pads the cents place"
  assert.eq failed
    actual:   "$19.5"
    expected: "$19.05"
  --> lib/money/test/cents.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

A suite that never compiled has no cases, so the report counts it separately.
Tests are otherwise ordinary build actions: a suite whose sources, target,
dependencies and toolchain are unchanged does not run again and reports as
**cached**. The runner shards and reorders freely, and no flag turns that off.

A suite runs natively on the host. Only `test { platforms: [JS] }` or
`--output=js` sends it to JavaScript. A program the backend has no body for, or
a toolchain that cannot build for this host, is an **error**
(`native-run-not-available` or `platform-not-implemented`), never a reroute.

Suites naming no platform go into one binary per tag-compatible batch, linked
once. Verdicts, caching and reports stay per suite. A `test { platforms }`,
`timeout_seconds` or `--output=` keeps a suite out of a batch.

## Lint findings about tests

`empty-test-suite` (a `test` block with no `sources`),
`test-without-assertion` (nothing reachable from the test calls into
`core/testing/assert` — transitive, so asserting through a helper is fine),
`test-title-newline`, and at run time `test-timeout`, `platform-not-implemented`
and `native-run-not-available`.
