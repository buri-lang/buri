# Testing your code

A test lives in the package it tests. That package's build rule declares it, and
it can reach exactly what a dependent could reach. There is no mocking
framework, because there is nothing for one to do: an effect arrives as an
argument, so a test that wants a different filesystem passes one.

The exact rules are in
[`reference/build/testing.md`](../reference/build/testing.md): the `test`
block's fields, what a test source may import, and every member of
`core/host/testing`.

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

Listing a file in `test.sources` is the whole of what makes it a test source.
`test` declarations are legal there and nowhere else, the compiler puts the file
in a test binary rather than in the library, and `buri gen` keeps the list in
step with the directory.

## Your first test

```buri repo=cli/tests/example role=test
// lib/money/test/parse.buri

from "core/testing/assert" import * as assert;
from "//lib/money" import { fromCents, parse, ParseError };

test "parses dollars and cents" {
    assert.equal(assert.ok(parse("19.05")), fromCents(1905));
}

test "rejects text that is not a number" {
    assert.equal(assert.err(parse("nineteen")), ParseError.NotANumber {
        text: "nineteen",
    });
}
```

Run it:

```sh
buri test //lib/money
```

A test takes no parameters and returns nothing. It passes by reaching the end
with no assertion failing. The first failure inside it ends that test alone, and
the rest of the file still runs. A failure reports the title, and `--filter`
matches on it, so two tests in one file may not share one.

Notice the import: the suite reaches its own library through `//lib/money`, the
name a dependent writes, and not through `//lib/money/parse.buri`. Reaching for
an internal module is an error, so a test asserts on what dependents can call and
refactoring behind the surface never rewrites one.

A binary's suite works the same way, except that its entry point is a file:
`from "//cmd/server/main.buri" import { route };`. `main` itself is not testable,
because it builds its own context out of `core/host` — which is the pressure that
keeps logic in functions taking an ordinary `ctx`.

## Assertions

`core/testing/assert` is an ordinary module, and `assert` is the name this file
gave it. Two kinds of function live in it:

| | |
|---|---|
| `assert.equal`, `assert.notEqual`, `assert.isTrue`, `assert.isFalse`, `assert.contains`, `assert.isEmpty`, `assert.notEmpty`, `assert.length`, `assert.greaterThan`, `assert.greaterOrEqual`, `assert.lessThan`, `assert.lessOrEqual`, `assert.approximatelyEqual` | Answer `()`, so they stand alone as statements |
| `assert.ok`, `assert.err`, `assert.some` | Answer the unwrapped value, which is how a test consumes a `Result` or an `Option` |

Reach for the narrowest one: every assertion in the first row names the two
values it compared, while `assert.isTrue(xs.contains(x))` reports only "expected
true, got false". The second kind makes a test read forwards — unwrap, then
assert on what came out.

```buri repo=cli/tests/example role=test
# from "core/testing/assert" import * as assert;
# from "//lib/money" import { parse, ParseError };

test "the error says which text it choked on" {
    let e = assert.err(parse("nineteen"));
    assert.equal(e, ParseError.NotANumber { text: "nineteen" });
}
```

A type an assertion compares needs `Equal`, and one a failure prints needs `Show`,
so `derive Equal, Show for ParseError;` is what lets you write both lines above.
`Result` is must-use in tests too, so a test cannot silently skip the check it
looks like it makes.

## A test needs a context exactly when the code does

`parse` is pure, with no `ctx` parameter, so its suite builds no context at all.
`format` allocates, says so with `C: Allocator`, and its test has to supply one:

```buri repo=cli/tests/example role=test
from "core/effect" import { Allocator };
from "core/host/testing" import { alloc };
from "core/testing/assert" import * as assert;
from "//lib/money" import { fromCents };

test "pads the cents place" {
    let ctx = context {
        Allocator: alloc(),
    };
    assert.equal(fromCents(1905).format(ctx), "$19.05");
}
```

That is the same `context` form an entry uses: a test source and an entry's body
are the only places that *create* a context rather than receive one, and only a
test source may import `core/host/testing`. Bind what the function under test needs
and nothing more — a context that does not name `Network` proves that nothing it
calls, however deep, reaches the network.

## Doubles are values, not a framework

Every member of `core/host/testing` is a function, and each call mints a fresh
double, so nothing leaks from one test to the next. The defaults fail loudly
rather than plausibly:

- `fs()` is an empty in-memory filesystem. One double answers both `FileSystemRead` and
  `FileSystemWrite`, so a context that reads and writes binds the *same* value under
  both names.
- `net()` refuses every request.
- `clock()` is stopped at zero.
- `rand()` is seeded.
- `stdin()` is at end of input.
- `stdout()` captures rather than prints.

Configure one by calling a builder on it, which answers a *new* double and
leaves the old one alone. Read the environment back at the end of the test:

```buri role=test
from "core/effect" import { Allocator, IoError };
# from "core/fs" import * as fs;
from "core/fs" import { FileSystemRead, FileSystemWrite, Path };
from "core/host/testing" import { alloc, fs };
from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

# fn archive<C: Allocator + FileSystemRead + FileSystemWrite>(
#     ctx: C,
#     at: Path,
# ): Result<(), IoError> {
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
    // One filesystem, bound under both names: `FileSystemRead: fs(), FileSystemWrite: fs()`
    // would be two of them, and the copy would land in the one nobody read.
    let disk = fs().files([("notes.txt", "hello")]);
    let ctx = context {
        Allocator: alloc(),
        FileSystemRead: disk,
        FileSystemWrite: disk,
    };
    assert.ok(archive(ctx, path.of(ctx, "notes.txt")));
    assert.equal(disk.snapshot(), [("notes.txt", "hello"), ("notes.txt.bak", "hello")]);
}

test "a read-only filesystem refuses the write, and nothing is written" {
    let disk = fs().files([("notes.txt", "hello")]);
    let refused = disk.readOnly();
    let ctx = context {
        Allocator: alloc(),
        FileSystemRead: refused,
        FileSystemWrite: refused,
    };
    assert.equal(assert.err(archive(ctx, path.of(ctx, "notes.txt"))), .ReadOnly);
    assert.equal(disk.snapshot(), [("notes.txt", "hello")]);
}
```

The outcome of a test is what the function answered *plus* what the environment
holds afterwards. `snapshot()` reads a filesystem back, `captured()` reads a
stream back, and `calls()` lists what the code under test asked for. None of the
three needs its effect bound.

### Writing your own

A double the runner does not provide is a struct with methods, bound in a
context the way the runner's own are:

```buri role=test
# from "core/effect" import { Allocator, NetError, Network, Request, Response };
# from "core/host/testing" import { alloc };
# from "core/net/http" import * as http;
# from "core/testing/assert" import * as assert;

# fn status<C: Network>(ctx: C, url: Str): Result<Int, NetError> {
#     http.send(ctx, http.request(.Get, url)).map(fn(r) => r.status)
# }

struct StubNet {
    export failing: Str,
}

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
    let ctx = context {
        Allocator: alloc(),
        Network: StubNet { failing: "https://example.test/slow" },
    };
    assert.equal(
        assert.err(status(ctx, "https://example.test/slow")),
        NetError.Timeout,
    );
    assert.equal(assert.ok(status(ctx, "https://example.test/x")), 200);
}
```

A fake you write answers from its fields rather than from a counter, because
there is no mutation to keep a counter in; the runner's own doubles are the ones
that record. For `Network` you rarely need this at all —
`net().respond(fn(request) => ...)` is already written.

## What breaks: fault plans

Fixtures say what a call *finds*. A fault plan says what a call **fails
with**:

```buri role=test
from "core/effect" import { Allocator };
# from "core/fs" import * as fs;
from "core/fs" import { FileSystemRead };
from "core/host/testing" import { alloc, fs, readFile };
from "core/path" import * as path;
# from "core/testing/assert" import * as assert;

test "a file that cannot be read is reported rather than skipped" {
    let disk = fs()
        .files([("config.toml", "name = \"demo\"")])
        .faults([readFile("config.toml").fails(.PermissionDenied)]);
    let ctx = context {
        Allocator: alloc(),
        FileSystemRead: disk,
    };
    let at = path.of(ctx, "config.toml");
    assert.equal(assert.err(fs.readText(ctx, at)), .PermissionDenied);
}
```

You spell a fault as the call it names: `readFile(path)`, `writeFile(path,
body)`, `fetch(request)`. The path there is the `Str` a `Path` spells, which is
what `calls()` reports it as. `fails(e)` fails every occurrence, and
`failsOnCall(n, e)` fails the `n`th. **A fault whose call never happens fails
the test**, so a plan cannot quietly stop describing the code.

A fault fails one call and says nothing about the state between two of them. For
code that must survive a crash, split each durable operation into a pure
`prepare`, one effectful `persist` that writes once, and a pure `publish` that
folds the outcome back in. A test calls the pure halves directly.

## Ordering, and the seed that replays it

`Tasks.parallel` promises its results in the items' order and nothing about the
order the work runs in. `tasks()` makes that second order a value the test
writes: program order by default, `anyOrder()` to let the suite pick one,
`seed(n)` to name one, `everyOrder()` to run the body under all of them. Use
`anyOrder()` to *find* an order that breaks a program, and the report names the
seed:

```text
FAIL //lib/merge  test/merge.buri  "the merge does not depend on which read finished first"
  assert.equal failed
    actual:   .Configuration { name: "demo", token: "" }
    expected: .Configuration { name: "demo", token: "abc" }
  the tasks completed in the order 1, 0 — replay it with `tasks().seed(1)`
  --> lib/merge/test/merge.buri:14:1
```

Paste the `seed(...)` back over the `anyOrder()` and you replay the run that
failed. Keep it that way: a test asserting on an order names the order it
means.

## Golden values and fixture files

A golden is a value in the suite's own source, compared with `assert.equal`. There
is no `--accept`: you rewrite one in your editor, and a diff review approves it.
Hand a test a filesystem only when the code under test is what does the reading,
as in `fs().files([("statement.txt", "coffee")])` — one holding an expected
string is a golden written the hard way.

A picture is the exception, because nobody hand-writes a PNG. `ui/testing`'s
`snapshot` compares what a tree paints against a golden in
`test/__snapshots__/` beside the suite, and `buri test --update` records it. The
[user interfaces guide](user-interfaces.md#snapshots) has the rest.

## Fixtures more than one suite wants

A helper two suites need is not a test source. It is ordinary library code that
happens to be test-only, behind a path with a `testing` segment:

```buri repo=cli/tests/example package=//lib/ledger role=testing
# from "core/effect" import { Allocator };
# from "core/fs" import { FileSystemRead };
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
    Allocator: alloc(),
    FileSystemRead: fs().files([("ledger.log", "coffee\t$4.50\n")]),
}
```

A named context binds `FileSystemRead` and not `FileSystemWrite` because its bindings are
separate expressions: naming both halves would call `fs()` twice and hand every
suite two unrelated filesystems. A fixture that must be written to as well as
read is a function answering the double.

A `testing { sources: [...] }` block puts it in the build, and a consuming suite
names `//lib/ledger/testing` in its own `test.dependencies`. Reach for this the
moment a second suite wants the fixture, and not before: a fixture on a public
surface is an API.

## Running

```sh
buri test //...                      every test in the repository
buri test //lib/money                one package's suites
buri test //lib/money --filter=pads  only tests whose title contains "pads"
buri test //... --watch              re-run on every save, until interrupted
```

`--filter` is a substring match on the test's title, which is the other reason
to write titles carefully. `--watch` re-runs the same invocation whenever a
declared input changes. A file you have just created is an input of nothing, so
run `buri gen` and the loop picks it up with the build file.

A failure names the target, the file, the test, and the two values that did not
match. The runner skips a suite whose declared inputs have not moved, and the
summary counts what was cached.
[The exact shape of a run](../reference/build/testing.md#running) is in the
reference. `buri test` exits `0` only when every test passed, so it works as a
CI gate.

## Next

- [`buri docs cli test`](../reference/cli/test.md) — flags, the watch loop, and
  where a suite runs.
- [Effects and capabilities](./effects.md) — why a bound is what confines a
  test.
