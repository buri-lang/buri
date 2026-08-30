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
        data: ["test/golden/statement.txt"]
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

The first five return `()`, so they stand alone as statements — a test source
is the one place the language admits an expression statement, and only when the
type is `()`. Any expression of that type qualifies, not only a call: a `match`
whose arms all assert is a statement too, terminated by `;` like the rest. The
last three return a value, and are how a `Result` is consumed in a test, since
`Result` is still must-use here.

```buri
test "reads the config it wrote" {
    let ctx = Hermetic();
    assert.ok(fs.writeText(ctx, "cfg", "port=8080"));   // returns (), so a statement
    let text = assert.ok(fs.readText(ctx, "cfg"));      // returns Str, so a binding
    assert.eq(text, "port=8080");
}
```

If `assert.eq` reports `unsatisfied-bound`, the type under test is missing
`derive Eq, Show for ThatType;` in **its own** module.

## The runner's context

`core/testing/context` exports one implementation per effect, not one
pre-assembled world. It is importable only from a test source.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Alloc` | real, from a per-test arena the runner reclaims |
| `captureOut()`, `captureErr()` | `Stdout`, `Stderr` | captured and never printed; `captured()` reads it back |
| `stdin([Str])` | `Stdin` | these lines, then end-of-input |
| `data()` | `Fs` | in-memory, rooted at the package directory, containing exactly `test { data }` |
| `files([(Str, Str)])` | `Fs` | in-memory, containing exactly these entries |
| `readOnly(F)` | `Fs` | wraps an `Fs` so every write fails |
| `noNet()` | `Net` | refuses every connection |
| `clockAt(Int)` | `Clock` | starts there, advances only when the test advances it |
| `randSeed(Int)` | `Rand` | seeded, so a failure reproduces |
| `envOf([(Str, Str)], [Str])` | `Env` | these variables and these arguments |

`Hermetic` binds all of them at hermetic defaults — empty `envOf`,
`clockAt(0)`, `randSeed(0)`, `data()` for the filesystem. A file may use it
directly, declare its own on top of it, or build one per test:

```buri
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

**Each call builds a fresh context**, which is why a named context is called
rather than referred to: what one test writes to its filesystem or its captured
stdout is invisible to the next.

## Fakes

A test double is an ordinary struct with methods, because effects are ordinary
interfaces. There is no mocking framework and no global to stub.

```buri
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
}
```

A fake answers from its fields rather than from a counter — there is no
mutation to hold one in. Keeping state between calls is the runner's own
privilege: `clockAt` and `captureOut` are intrinsics holding a slot in a table
the runtime owns, and a fake you write cannot get one. For "the third write
fails", split the operation into a pure `prepare`, one effectful `persist` and a
pure `publish`, and let the test hand the step it chooses an `.Err`.

This is defence in depth. The primary mechanism is that a suite whose calls
never passed a `Net`-bounded context cannot open a socket in anything it
transitively calls, so the toolchain applies no operating-system confinement.

## What a test source may and may not do

May import: the target under test (`//lib/money/lib.buri`, or
`//cmd/server/main.buri` for a binary), the target's `dependencies`, the suite's
`test.dependencies`, `core/*` including the test platform, and any test-only
path.

May **not**: import a library-internal module (`//lib/money/cents.buri` →
`test-internal-import`); import another test source (they are compiled
independently); be imported by anything; `export` anything.

If a test needs an internal function, either the function belongs on the
surface — say so in `lib.buri` — or the test is asserting on an implementation
detail.

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
    let ctx = context { ..Hermetic(), Fs: readOnly(data()) };
    let msg = assert.err(run(ctx, "ledger.log"));
    assert.isTrue(msg.contains("ledger"));
}
```

## Shared fixtures

A helper more than one suite needs is not a test source — it is ordinary
library code behind a path with a `testing` segment, declared by a
`testing { sources: [...] }` block. It may import the library's internals, has
its own `dependencies`, is never linked into a production artifact, and
inherits the library's `visibility` and `tags`. A consumer's suite reaches it
by label, declared in `test { dependencies }`.

Prefer a private helper while only one suite wants the fixture, and promote it
as soon as a second does — a fixture on a public surface is an API.

## Golden files

`test { data: [...] }` declares the files the in-memory `Fs` contains, so
`Hermetic()`'s filesystem already holds them:

```buri
test "renders the statement" {
    let ctx = Hermetic();
    let want = assert.ok(fs.readText(ctx, "test/golden/statement.txt"));
    assert.eq(render(ctx, sample()), want);
}
```

`buri test --accept` runs the suites outside the cache, collects the actual
value from every `assert.eq` whose expected side came from a declared `data`
file, and rewrites those files in the source tree. It never creates a file,
never touches one not listed in `data`, and prints a diff for each. The
ordinary `buri test` path stays hermetic and cacheable.

## Running

```
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  substring match on test names
buri test //... --output=js          send the suites that name no platform to JS
buri test //lib/money --accept       update declared golden files
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

A suite runs natively on the host unless something sends it to JavaScript: its
own `test { platforms }`, `--output=js`, `--accept`, or the fallback for a
program the native backend cannot yet take, which prints one line on stderr per
suite. A platform this toolchain cannot build for is an **error** when a suite
named it and a fallback when nobody did.

Suites that name no platform are compiled into one binary per tag-compatible
batch and linked once. Verdicts, caching and reports are still per suite; a
`test { platforms }`, `test { data }`, `timeout_seconds`, `--output=` or
`--accept` keeps a suite out of a batch.

## Lint findings about tests

`empty-test-suite` (a `test` block with no `sources`),
`test-without-assertion` (nothing reachable from the test calls into
`core/testing/assert` — it is transitive, so asserting through a helper is
fine), `test-title-newline`, and at run time `test-timeout` and
`platform-not-implemented`.
