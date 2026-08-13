# Testing

Tests live inside the target they test, are declared in its build rule, and can
reach only what a dependent could reach. There is no separate test target, no
test-only source directory outside the package, and no way to test a private
function directly.

The language side of this — the `test` declaration, `core/testing/assert`, and
`context()` — is [`SPEC.md` §11.2](../SPEC.md). This document is the build
system's half: where tests live, what they may import, and how they run.

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

```textproto
library {
  name: "money"
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

```buri
// lib/money/test/cents.buri

from "//lib/money" import { fromCents, fromDollars };
from "core/testing/assert" import * as assert;
from "core/testing/context" import { context };

test "pads the cents place" {
  let ctx = context();
  assert.eq(fromCents(1905).format(ctx), "\$19.05");
}

test "addition composes" {
  assert.eq(fromDollars(19).add(fromCents(499)), fromCents(2399));
}
```

A test takes no parameters and returns nothing. It passes unless an assertion in
it fails, and a failing assertion ends that test and no other.

A test that needs a context builds one — `context()` is the one place in
the language where a context is created rather than received, which is why
`core/testing/context` is importable only from a test source. A pure assertion,
like the second one above, needs no context at all, and the fact that you can see which
is which from the test body is the point of the effect system showing up here
too.

`core/testing/assert` is an ordinary module — the name `assert` comes from
`import * as assert`, and nothing stops a file calling it something else.
`assert.eq`, `assert.notEq`, `assert.isTrue`, `assert.isFalse`, and
`assert.fail` return `()`, so they stand alone as statements, which is the one
place the language admits an expression statement. `assert.ok`, `assert.err`,
and `assert.some` return the unwrapped value, which is how a `Result` is
consumed in a test:

```buri
test "rejects text that is not a number" {
  let e = assert.err(parse("nineteen"));
  assert.eq(e, ParseError.NotANumber { text: "nineteen" });
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

- import a library-internal module — `from "//lib/money/cents" import
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
 3 | from "//lib/money/cents" import { toCents };
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
`testing` segment ([`LIBRARIES.md`](./LIBRARIES.md#the-testing-surface)):

```buri
// lib/ledger/testing/fixtures.buri — inside //lib/ledger, so it can use the
// library's internals to build a fixture.

from "//lib/ledger/entry" import { Entry, entry };
from "//lib/money" import { fromCents, fromDollars };

/// A three-entry ledger, one of them zero, for anyone testing against ledgers.
export fn sample(): [Entry] {
  [
    entry("coffee", fromCents(450)),
    entry("refund", fromCents(0)),
    entry("books", fromDollars(32)),
  ]
}
```

```textproto
# lib/ledger/BUILD.buri
library {
  name: "ledger"
  sources: ["entry.buri", "posting/rules.buri"]
  dependencies: ["//lib/money"]

  testing {
    sources: ["testing/fixtures.buri"]
  }

  test {
    sources: ["test/ledger.buri"]
  }
}
```

```buri
// tools/report/test/render.buri — a different package's suite, using it.
from "//lib/ledger/testing" import { sample };
```

with `test { dependencies: ["//lib/ledger/testing"] }` in that package's rule.
Importing it from a non-test source is an error, and the error is about the
path, so nobody has to have set a flag correctly for it to fire.

Prefer this to a private helper as soon as a second suite wants the same
fixture, and prefer a private helper while only one does — a fixture on a
public surface is an API, and it will be depended on.

## Testing a binary

A binary's `main.buri` is its surface, exactly as `lib.buri` is a library's, and
its test sources import it by its module path:

```buri
// cmd/server/main.buri

from "//cmd/server/routes" export { Route, route };

from "//cmd/server/routes" import { route };
from "core/cap" import { Alloc, Stdout };

export fn main<C: Alloc + Stdout>(ctx: C): Result<(), Str> {
  let _ = ctx.println("listening on ${route("/entries").name}");
  .Ok(())
}
```

```buri
// cmd/server/test/routes.buri

from "//cmd/server/main" import { route };

test "unknown paths route to the fallback" {
  assert.eq(route("/nope").name, "fallback");
}
```

`//cmd/server/main` is a module inside the package, so only that package's own
test sources may import it — the same rule that keeps `//lib/money/cents`
internal. Everything else is identical to testing a library: the test sees what
`main.buri` exports and nothing more, so pushing logic behind the entry point is
what makes it testable, which is the pressure you want.

Testing `main` itself is possible and usually not what you want:

```buri
test "main fails cleanly when the log is unwritable" {
  let ctx = context().withReadOnlyFs();
  let msg = assert.err(main(ctx));
  assert.isTrue(msg.contains("ledger"));
}
```

## The runner's context

`context()` returns a value satisfying every effect the runner can grant.
It is real where it can be, and hermetic everywhere else:

| Effect | In a test |
|---|---|
| `Alloc` | Real, with a per-test arena the runner reclaims. |
| `Stdout`, `Stderr` | Captured. Printed only for a failing test. |
| `Fs` | In-memory, rooted at the package directory, containing exactly `test.data`. Writes are visible to that test and discarded after it. |
| `Net` | Refuses every connection. A fake goes in `test.dependencies`. |
| `Clock` | Starts at a fixed instant and advances only when the test advances it. |
| `Rand` | Seeded from the test's name, so a failure reproduces. |
| `Env` | Empty, unless the test adds to it. |

Builders narrow or populate it — `context().withFiles([...])`,
`.withArgs([...])`, `.withReadOnlyFs()` — and anything the runner does not
provide is an ordinary struct with methods, since effects are ordinary
interfaces ([`SPEC.md` §10.9](../SPEC.md)):

```buri
struct FlakyNet { export failuresLeft: Int }

fn get(self: FlakyNet, url: Str): Result<Response, NetError> {
  if (self.failuresLeft > 0) { .Err(.Timeout) } else { .Ok(cannedResponse()) }
}

test "retries a timeout once" {
  let ctx = context().withNet(FlakyNet { failuresLeft: 1 });
  assert.ok(fetchWithRetry(ctx, "https://example.test/x"));
}
```

This is defence in depth rather than the primary mechanism. The primary
mechanism is that a test whose call never passed a `Net`-bounded context cannot
open a socket in anything it transitively calls — that is
[`SPEC.md` §10](../SPEC.md), not a build system feature. The sandbox in
[`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md) is the third layer,
covering the compiler and the runner themselves.

## Test data and golden files

`test { data: [...] }` declares the files the in-memory `Fs` contains:

```textproto
test {
  sources: ["test/ledger.buri"]
  data: ["test/golden/statement.txt"]
  timeout_seconds: 30
}
```

```buri
test "renders the statement" {
  let ctx = context();
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
buri test //... --output=js          run every suite through the JS backend
buri test //lib/money --accept       update declared golden files
```

Output names the target, the file, and the test:

```
FAIL //lib/money  test/cents.buri  "pads the cents place"
  assert.eq failed
    actual:   "$19.5"
    expected: "$19.05"
  --> lib/money/test/cents.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

Tests are ordinary build actions: a suite whose sources, target, dependencies,
and toolchain are unchanged is not re-run, and reports as cached. Because there
is no mutable global state, no ambient I/O, and no observable ordering, the
runner is free to shard across processes and to run a suite's tests in any
order — `--shuffle` is on by default and the seed is printed.
