# Testing your code

A test lives in the package it tests, is declared in that package's build rule,
and can reach exactly what a dependent could reach. There is no mocking
framework, because there is nothing for one to do: an effect arrives as an
argument, so a test that wants a different filesystem passes a different
filesystem.

This page teaches writing a suite. The exact rules — the `test` block's fields,
what a test source may import, and every member of `core/host/testing` — are in
[`reference/build/testing.md`](../reference/build/testing.md).

## Declare the suite

Tests go in `test/`, beside the code, and the rule lists them:

```textproto schema=build
library {
    sources: ["cents.buri", "parse.buri"]

    test {
        sources: ["test/cents.buri", "test/parse.buri"]
    }
}
```

Being listed in `test.sources` is the whole of what makes a file a test source.
`test` declarations are legal there and nowhere else, the file is compiled into
a test binary rather than into the library, and `buri gen` keeps the list in
step with the directory. Nothing else in the package changes.

## Your first test

```buri repo=cli/tests/example role=test
// lib/money/test/parse.buri

from "core/testing/assert" import * as assert;
from "//lib/money" import { fromCents, parse, ParseError };

test "parses dollars and cents" {
    assert.eq(assert.ok(parse("19.05")), fromCents(1905));
}

test "rejects text that is not a number" {
    assert.eq(assert.err(parse("nineteen")), ParseError.NotANumber {
        text: "nineteen",
    });
}
```

Run it:

```sh
buri test //lib/money
```

A test takes no parameters and returns nothing: it passes by getting to the end
without an assertion failing, and the first failure inside it ends that test
alone — the rest of the file still runs. A failure reports the title, and
`--filter` matches it, so two tests in one file may not share one.

Notice the import: the suite reaches its own library through `//lib/money` — the
surface, the same name a dependent writes — and not through
`//lib/money/parse.buri`. You test what dependents can call, so refactoring
behind the surface never rewrites a test. Reaching for an internal module is an
error, and the fix is either to put the name on the surface or to stop asserting
on a detail.

A binary's suite works the same way, except that its entry point is a file:
`from "//cmd/server/main.buri" import { route };`. `main` itself is not
testable — it builds its own context out of `core/host`, so there is no double
to hand it — which is the pressure that keeps logic in functions taking an
ordinary `ctx`.

## Assertions

`core/testing/assert` is an ordinary module; `assert` is the name this file gave
it with `import * as assert`. Two kinds of function live in it:

| | |
|---|---|
| `assert.eq`, `assert.notEq`, `assert.isTrue`, `assert.isFalse`, `assert.contains`, `assert.isEmpty`, `assert.notEmpty`, `assert.len`, `assert.gt`, `assert.ge`, `assert.lt`, `assert.le`, `assert.approxEq` | Answer `()`, so they stand alone as statements |
| `assert.ok`, `assert.err`, `assert.some` | Answer the unwrapped value, which is how a test consumes a `Result` or an `Option` |

Reach for the narrowest one: every assertion in the first row names the two
values it compared, and `assert.isTrue(xs.contains(x))` reports only "expected
true, got false".

The second kind is what makes a test read forwards: unwrap, then assert on what
came out.

```buri repo=cli/tests/example role=test
# from "core/testing/assert" import * as assert;
# from "//lib/money" import { parse, ParseError };

test "the error says which text it choked on" {
    let e = assert.err(parse("nineteen"));
    assert.eq(e, ParseError.NotANumber { text: "nineteen" });
}
```

A type an assertion compares needs `Eq`, and one a failure prints needs `Show`,
so `derive Eq, Show for ParseError;` is what lets both lines above be written.
`Result` is must-use everywhere, tests included: there is no statement form that
drops one, so a test cannot silently skip the check it looks like it makes.

## A test needs a context exactly when the code does

`parse` is pure — no `ctx` parameter — so its suite builds no context at all.
`format` allocates, says so with `C: Alloc`, and its test has to supply one:

```buri repo=cli/tests/example role=test
from "core/effect" import { Alloc };
from "core/host/testing" import { alloc };
from "core/testing/assert" import * as assert;
from "//lib/money" import { fromCents };

test "pads the cents place" {
    let ctx = context {
        Alloc: alloc(),
    };
    assert.eq(fromCents(1905).format(ctx), "$19.05");
}
```

That is the same `context` form `main` uses, and a test source and `main`'s body
are the only places in the language where a context is *created* rather than
received. `core/host/testing` is the platform a test binds, importable only from
a test source.

Bind what the function under test needs and nothing more. A context that names
`Net` is a test admitting the code might reach the network; a context that does
not name it is a proof that nothing it calls, however deep, can.

## Doubles are values, not a framework

Every member of `core/host/testing` is a function you call, and each call mints
a fresh double, so nothing leaks from one test to the next. The defaults fail
loudly rather than plausibly:

- `fs()` is an empty in-memory filesystem — one double answering both `FsRead`
  and `FsWrite`, so a context that reads and writes binds the *same* value
  under both names.
- `net()` refuses every request.
- `clock()` is stopped at zero.
- `rand()` is seeded.
- `stdin()` is at end of input.
- `stdout()` captures rather than prints.

Configure one by calling a builder on it, which answers a *new* double and
leaves the old one alone. Then read the environment back at the end of the test:

```buri role=test
from "core/effect" import { Alloc, IoError };
# from "core/fs" import * as fs;
from "core/fs" import { FsRead, FsWrite, Path };
from "core/host/testing" import { alloc, fs };
from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn archive<C: Alloc + FsRead + FsWrite>(ctx: C, at: Path): Result<(), IoError> {
#     match (at.withSuffix(ctx, ".bak")) {
#         .None => .Err(.NotFound),
#         .Some(backup) => {
#             match (fs.readText(ctx, at)) {
#                 .Err(e) => .Err(e),
#                 .Ok(body) => fs.writeText(ctx, backup, body),
#             }
#         },
#     }
# }

test "archiving leaves the original alone and writes the copy beside it" {
    // One filesystem, bound under both names: `FsRead: fs(), FsWrite: fs()`
    // would be two of them, and the copy would land in the one nobody read.
    let disk = fs().files([("notes.txt", "hello")]);
    let ctx = context {
        Alloc: alloc(),
        FsRead: disk,
        FsWrite: disk,
    };
    assert.ok(archive(ctx, path.of(ctx, "notes.txt")));
    assert.eq(disk.snapshot(), [("notes.txt", "hello"), ("notes.txt.bak", "hello")]);
}

test "a read-only filesystem refuses the write, and nothing is written" {
    let disk = fs().files([("notes.txt", "hello")]);
    let refused = disk.readOnly();
    let ctx = context {
        Alloc: alloc(),
        FsRead: refused,
        FsWrite: refused,
    };
    assert.eq(assert.err(archive(ctx, path.of(ctx, "notes.txt"))), .ReadOnly);
    assert.eq(disk.snapshot(), [("notes.txt", "hello")]);
}
```

The outcome of a test is what the function answered *plus* what the environment
holds afterwards. `snapshot()` reads a filesystem back, `captured()` reads a
stream back, and `calls()` lists what the code under test actually asked for.
None of the three needs its effect bound.

### Writing your own

An effect is an ordinary interface, so a double the runner does not provide is a
struct with methods, bound in a context the way the runner's own are:

```buri role=test
# from "core/effect" import { Alloc, Net, NetError, Request, Response };
# from "core/host/testing" import { alloc };
# from "core/net/http" import * as http;
# from "core/testing/assert" import * as assert;

# fn status<C: Net>(ctx: C, url: Str): Result<Int, NetError> {
#     http.send(ctx, http.request(.Get, url)).map(fn(r) => r.status)
# }

struct StubNet {
    export failing: Str,
}

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
    let ctx = context {
        Alloc: alloc(),
        Net: StubNet { failing: "https://example.test/slow" },
    };
    assert.eq(assert.err(status(ctx, "https://example.test/slow")), NetError.Timeout);
    assert.eq(assert.ok(status(ctx, "https://example.test/x")), 200);
}
```

Nothing was registered, patched, or injected; the call site of `status` is the
same one production uses. A fake you write answers from its fields rather than
from a counter, because there is no mutation to keep one in — the runner's own
doubles are what record and accumulate. For `Net` in particular you rarely need
this: `net().respond(fn(request) => ...)` is the same function of the same
request, already written.

## What breaks: fault plans

Fixtures say what a call *finds*. A fault plan says what a call **fails with**,
and those are the only two sources — so a reader of a failing test knows which
half to look in:

```buri role=test
from "core/effect" import { Alloc };
# from "core/fs" import * as fs;
from "core/fs" import { FsRead };
from "core/host/testing" import { alloc, fs, readFile };
from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

test "a file that cannot be read is reported rather than skipped" {
    let disk = fs()
        .files([("config.toml", "name = \"demo\"")])
        .faults([readFile("config.toml").fails(.PermissionDenied)]);
    let ctx = context {
        Alloc: alloc(),
        FsRead: disk,
    };
    let at = path.of(ctx, "config.toml");
    assert.eq(assert.err(fs.readText(ctx, at)), .PermissionDenied);
}
```

A fault is spelled as the call it names — `readFile(path)`, `writeFile(path,
body)`, `fetch(request)` — where the path is the `Str` a `Path` spells, which is
what `calls()` reports it as. `fails(e)` fails every occurrence and
`failsOnCall(n, e)` fails the `n`th. **A fault whose call never happens fails
the test**, so a plan cannot quietly stop describing the code.

A fault fails one call and says nothing about the state between two of them. For
code that must survive a crash, split each durable operation into a pure
`prepare`, one effectful `persist` that writes once, and a pure `publish` that
folds the outcome back in. The pure halves take no context, so a test calls them
directly, and the recovery path becomes ordinary values rather than something to
interrupt mid-call.

## Ordering, and the seed that replays it

`Tasks.parallel` promises its results in the items' order and promises nothing
about the order the work runs in. `tasks()` makes that second order a value the
test writes: program order by default, `anyOrder()` to let the suite pick one,
`seed(n)` to name one, `everyOrder()` to run the body under all of them.

Use `anyOrder()` to *find* an order that breaks a program. When one does, the
report names the seed:

```text
FAIL //lib/merge  test/merge.buri  "the merge does not depend on which read finished first"
  assert.eq failed
    actual:   .Configuration { name: "demo", token: "" }
    expected: .Configuration { name: "demo", token: "abc" }
  the tasks completed in the order 1, 0 — replay it with `tasks().seed(1)`
  --> lib/merge/test/merge.buri:14:1
```

Paste the `seed(...)` back over the `anyOrder()` and the run is the one that
failed. Keep it that way: a test asserting on an order should name the order it
means.

## Golden values and fixture files

A golden is a value in the suite's own source, compared with `assert.eq`. There
is no `--accept` and no golden directory; an editor is what rewrites one, and a
diff review is what approves it.

Hand a test a filesystem when the code under test is what does the reading —
`fs().files([("statement.txt", "coffee")])` — and not merely to hold an expected
string, which is a golden the filesystem is doing nothing for.

## Fixtures more than one suite wants

A helper two suites need is not a test source; it is ordinary library code that
happens to be test-only, and it lives behind a path with a `testing` segment:

```buri repo=cli/tests/example package=//lib/ledger role=testing
# from "core/effect" import { Alloc };
# from "core/fs" import { FsRead };
# from "core/host/testing" import { alloc, fs };

// lib/ledger/testing/fixtures.buri — inside //lib/ledger, so it can use the
// library's internals to build a fixture.

from "//lib/ledger/entry.buri" import { Entry, entry };
from "//lib/money" import { fromCents, fromDollars };

/// A three-entry ledger, one of them zero, for anyone testing against ledgers.
export fn sample(): [Entry] {
    [
        entry("coffee", fromCents(450)),
        entry("refund", fromCents(0)),
        entry("books", fromDollars(32)),
    ]
}

/// A context whose filesystem already holds a ledger, for suites that would
/// otherwise write the same three lines.
export context WithLedger {
    Alloc: alloc(),
    FsRead: fs().files([("ledger.log", "coffee\t$4.50\n")]),
}
```

A named context binds `FsRead` and not `FsWrite` for a structural reason: its
bindings are separate expressions, so naming both halves would call `fs()` twice
and hand every suite two unrelated filesystems. A fixture that must be written
to as well as read is a function answering the double, and the suite builds the
`context` expression where a `let` can hold it.

A `testing { sources: [...] }` block puts it in the build, and a consuming suite
names `//lib/ledger/testing` in its own `test.dependencies`. Reach for this the
moment a second suite wants the fixture, and not before: a fixture on a public
surface is an API, and it will be depended on.

## Running

```sh
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  only tests whose title contains "pads"
buri test //... --watch              re-run on every save, until interrupted
```

`--filter` is a substring match on the test's title, which is the other reason
titles are worth writing carefully. `--watch` re-runs the same invocation
whenever a declared input changes; a file you have just created is an input of
nothing, so run `buri gen` and the loop picks it up with the build file.

A failure names the target, the file, the test, and the two values that did not
match, and the summary counts what was cached — a suite whose declared inputs
have not moved is not re-run at all, which is what makes a watch loop cheap.
[The exact shape of a run](../reference/build/testing.md#running) is in the
reference.

`buri test` exits `0` only when every test passed, so it is usable directly as a
CI gate.

## Next

- The exact rules: [test targets and the testing host](../reference/build/testing.md)
  — the `test` block's fields, the import table, every double and builder, and
  what the runner does with a batch of suites.
- [`buri docs cli test`](../reference/cli/test.md) — flags, the watch loop, and
  where a suite runs.
- [Effects and capabilities](./effects.md) — why a bound is what confines a
  test in the first place.
