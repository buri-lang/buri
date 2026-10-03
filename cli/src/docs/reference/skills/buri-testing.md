---
name: buri-testing
description: Use when writing, running, or debugging Buri tests — the test declaration, assertions, contexts and fakes, golden files, and what buri test reports.
---

# Buri: writing and running tests

Tests belong to the target they test and reach only what a dependent could:
there's no separate test target and no testing private functions. The
normative pages are
`buri docs build/testing` and `buri docs cli test`.

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
        backends: [NATIVE, JS]
    }
}
```

**Only a module listed in `test.sources` is a test source.** `test`
declarations and imports of test-only modules are legal there and nowhere else.
`buri gen` maintains `test.sources`.

## A test

```buri
from "//lib/money" import { fromCents, fromDollars };
from "core/testing/assert" import * as assert;
from "platform/effect/testing" import { alloc };
from "platform/effect" import { Allocator };

test "pads the cents place" {
    let ctx = context { Allocator: alloc() };
    assert.equal(fromCents(1905).format(ctx), "\$19.05");
}

test "addition composes" {
    assert.equal(fromDollars(19).add(fromCents(499)), fromCents(2399));
}
```

- A test takes no parameters and returns nothing. It passes unless an assertion
  fails, and a failing assertion ends only that test.
- **A title appears once per file** (`duplicate-test-name`); two files may
  share one.
- A pure assertion needs no context. `assert` isn't a keyword; the name comes
  from `import * as assert`.

### Assertions

| Function | Meaning |
|---|---|
| `assert.equal(a, b)` / `notEqual` | fails unless `a == b`; needs `Equal`, and `Show` for the message |
| `assert.isTrue(b)` / `isFalse` | on a `Bool` |
| `assert.contains(xs, x)`, `isEmpty(xs)` / `notEmpty`, `len(xs, n)` | on a list |
| `assert.greaterThan(a, b)` / `ge` / `lt` / `le`, `approximatelyEqual(a, b, tolerance)` | on an `Ordered`, and on `Float` within an absolute tolerance |
| `assert.ok(r)` | fails unless `r` is `.Ok`; **returns the wrapped value** |
| `assert.err(r)` | fails unless `r` is `.Err`; returns the error |
| `assert.some(o)` / `none(o)` | fails unless `o` is `.Some` / `.None`; `some` returns the wrapped value |

- Use the narrowest one. `assert.isTrue(xs.contains(x))` reports only
  "expected true, got false".
- There is no `assert.fail`. Assert on the value you have:
  `assert.equal(verdict, "settled")`, or `assert.none(o)`.
- `ok`, `err` and `some` return a value, which uses up a must-use `Result`. The
  rest return `()` and stand alone as statements, which only a test source
  allows (type `()`, ending in `;`).

```buri
test "reads the config it wrote" {
    let disk = memory();                            // one filesystem, two effects
    let ctx = context { Allocator: alloc(), FileSystemRead: disk, FileSystemWrite: disk };
    let cfg = path.of(ctx, "cfg");                  // core/fs takes a Path
    assert.ok(fs.writeText(ctx, cfg, "port=8080")); // returns (), so a statement
    let text = assert.ok(fs.readText(ctx, cfg));    // returns Str, so a binding
    assert.equal(text, "port=8080");
}
```

If `assert.equal` reports `unsatisfied-bound`, add
`derive Equal, Show for ThatType;` in the type's **own** module.

## The runner's context

`platform/effect/testing` holds the test doubles, named after the host's fields.
**Call** each one to get a fresh double. Only a test source may import it.

| Member | Effect | In a test |
|---|---|---|
| `alloc()` | `Allocator` | real, from a per-test arena the runner reclaims |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | captured and never printed; `captured()` reads either back |
| `stdin()` | `Stdin` | at end of input, so a suite never blocks on a pipe |
| `fs()` | `FileSystemRead`, `FileSystemWrite` | in-memory and empty; writes discarded after the test. One call is one filesystem answering **both** effects, so a context that reads and writes binds the same value under both names |
| `net()` | `Network` | refuses every request until `respond` says what to answer |
| `clock()` | `Clock` | at zero; `sleepMilliseconds` advances it without sleeping |
| `rand()` | `Random` | seeded at zero, so a failure reproduces |
| `env()` | `Environment` | no variables and no arguments |
| `proc()` | `Process` | absorbs the exit, so the test carries on |
| `tasks()` | `Tasks` | runs the tasks one at a time, in program order |

Configure a double with a **method that returns a new handle**, leaving the
original alone:

- `clock().at(n)`, `rand().seed(n)`, `fs().readOnly()`, `tasks().anyOrder()`
- `env().variables([...]).withArguments([...])`
- `stdin().lines(...)` or `.bytes(...)`, which replace
- `fs().files(...)` and `.filesBytes(...)`, which compose
- `net().respond(fn(Request) => ...)`
- `faults([...])` says what fails; a fault whose call never happens fails the test

Read back what happened with `captured()`, `fs().read(p)`, `fs().snapshot()` and
`calls()`, which lists what the code under test **asked** for.

```buri
context Fixture {
    Allocator: alloc(),
    Environment: env().variables([("LEDGER_LOG", "custom.log")]).withArguments(["--verbose"]),
}

test "reads the log path from the environment" {
    let ctx = Fixture();
    assert.equal(logPath(ctx), "custom.log");
}

test "falls back when the variable is unset" {
    let ctx = context { ..Fixture(), Environment: env() };
    assert.equal(logPath(ctx), "ledger.log");
}
```

**Each `Fixture()` call builds a fresh context**, so tests never see each
other's writes or output. A `context` declaration's two bindings are two `fs()`
calls, so code that reads *and* writes binds one named double, as in
"reads the config it wrote". Bind only what the function needs.

## Fakes

Effects are ordinary interfaces, so a fake is an ordinary struct with methods.
There's no mocking framework and no global to stub.

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
    assert.equal(assert.err(status(ctx, "https://example.test/slow")), NetError.Timeout);
}
```

- A fake has no mutation, so it can't count calls. "The third write fails" is a
  fault plan: `fs().faults([...])`.
- To test a crash *between* two calls, split the work into pure `prepare`,
  effectful `persist` and pure `publish`, and hand the chosen step an `.Err`.
- A read-only fake implements `FileSystemRead`: four methods, not twelve.
- A suite that never binds `Network` can't open a socket.

## What a test source may import

- **May import** the target (`//lib/money`, or `//cmd/server/main.buri` for a
  binary), its `dependencies`, the suite's `test.dependencies`, `core/*`
  including the test platform, and any test-only path.
- **May not import** a library-internal module (`//lib/money/cents.buri` is
  `test-internal-import`) or another test source, since each compiles
  independently. It can't be imported and can't `export`.

A test that needs an internal function either wants it in `lib.buri` or is
testing an implementation detail.

**You can't test `main` itself.** It takes a host only the CLI can build. Put
the logic in a function taking an ordinary bounded `ctx`:

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

A helper several suites share goes in a `testing { sources: [...] }` block, under
a `testing` path. It may import the library's internals, has its own
`dependencies`, inherits the library's `visibility` and `tags`, and never links
into production. Consumers list it in `test { dependencies }`. A public fixture
is an API.

## Golden files

Write the suite's filesystem in the suite with `fs().files`:

```buri
from "platform/effect/testing" import { alloc, fs as memory };

test "renders the statement" {
    let ctx = context { Allocator: alloc(), FileSystemRead: memory().files([("statement.txt", "coffee")]) };
    let want = assert.ok(fs.readText(ctx, path.of(ctx, "statement.txt")));
    assert.equal(render(ctx, sample()), want);
}
```

If the code under test doesn't read files, put the expected value in the
assertion instead.
`test { data: [...] }` and `buri test --accept` are retired.

## Running

```
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  substring match on test names
buri test //... --output=js          send the suites that name no backend to JS
buri test //... --watch              re-run on every change to a declared input
buri test //... --explain            one line per action: ran, or served by the cache
```

`buri test` exits `0` when every test passed and `1` otherwise.

```
FAIL //lib/money  test/cents.buri  "pads the cents place"
  assert.equal failed
    actual:   "$19.5"
    expected: "$19.05"
  --> lib/money/test/cents.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

- A suite that didn't compile is counted separately.
- An unchanged suite (sources, target, dependencies, toolchain) reports as
  **cached** without running.
- The runner always shards and reorders.
- Suites run natively unless `test { backends: [JS] }` or `--output=js`. A
  missing backend body or an unbuildable host is an **error**
  (`native-run-not-available`, `platform-not-implemented`), never a reroute.
- Suites naming no backend share one binary per tag-compatible batch; results
  stay per suite. `test { backends }`, `timeout_seconds` or `--output=` opts a
  suite out.

## Lint findings about tests

- `empty-test-suite`: a `test` block with no `sources`.
- `test-without-assertion`: nothing reachable from the test calls into
  `core/testing/assert`. It's transitive, so asserting through a helper is fine.
- `test-title-newline`.
- At run time: `test-timeout`, `platform-not-implemented` and
  `native-run-not-available`.
