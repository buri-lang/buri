# Testing

Tests live inside the target they test, are declared in its build rule, and can
reach only what a dependent could reach. There is no separate test target, no
test-only source directory outside the package, and no way to test a private
function directly.

```
lib/money/
  BUILD.buri
  lib.buri
  cents.buri
  format.buri
  test/
    cents.buri
    format.buri
```

```textproto
library {
  name: "money"
  srcs: ["cents.buri", "format.buri"]

  test {
    srcs: [
      "test/cents.buri",
      "test/format.buri",
    ]
  }
}
```

## The `test` declaration

The grammar gains one item form:

```ebnf
Item            ::= Import
                  | ReExport
                  | TestDecl
                  | Declaration

TestDecl        ::= "test" STRING "(" Params? ")" ":" Type Block
```

```buri
// lib/money/test/format.buri

from "//lib/money" import { fromCents, format };
from "core/cap" import { Alloc };
from "core/test" import { Expect };
from "core/test" import * as t;

test "pads the cents place" (ctx: { alloc: Alloc, expect: Expect }): Result<{}, Str> {
  t.eq(ctx, fromCents(1905).format(ctx), "\$19.05")?;
  .Ok({})
}

test "renders whole dollars with two zeros" (ctx: { alloc: Alloc, expect: Expect }): Result<{}, Str> {
  t.eq(ctx, fromCents(1900).format(ctx), "\$19.00")?;
  .Ok({})
}
```

A test is a function with a name that is a string rather than an identifier, and
the same shape as `main`:

- exactly one parameter, a record type — the context the runner must supply;
- return type exactly `Result<{}, Str>`;
- `.Ok({})` passes, `.Err(msg)` fails with `msg`.

The string name is deliberate. Test names are prose, they are read in failure
output, and encoding prose in an identifier produces
`test_pads_the_cents_place` and then arguments about the convention.

Reserving `test` as a keyword costs the identifier `test`; the alternatives were
a naming convention, which the compiler cannot check, and an attribute syntax,
which the language does not have. It is [open question
1](./README.md#open-questions).

## Assertions are `Result`

`core/test` is ordinary library code. Nothing about it is magic:

```buri
fn eq<A: Eq + Show>(ctx: { alloc: Alloc, expect: Expect, .. }, actual: A, expected: A): Result<{}, Str>
fn notEq<A: Eq + Show>(ctx: { alloc: Alloc, expect: Expect, .. }, actual: A, expected: A): Result<{}, Str>
fn isTrue(ctx: { expect: Expect, .. }, actual: Bool): Result<{}, Str>
fn isOk<A, E: Show>(ctx: { alloc: Alloc, expect: Expect, .. }, r: Result<A, E>): Result<A, Str>
fn isErr<A: Show, E>(ctx: { alloc: Alloc, expect: Expect, .. }, r: Result<A, E>): Result<E, Str>
fn fail(ctx: { alloc: Alloc, expect: Expect, .. }, msg: Str): Result<{}, Str>
```

Two properties fall out of the language and are worth naming, because in most
test frameworks they are bugs that are only found later:

**An assertion you forget to check does not compile.** `Result` is must-use
([`SPEC.md` §6.8](../SPEC.md)), so a bare `t.eq(ctx, a, b);` is not a statement
this language has, and `let _ = t.eq(ctx, a, b);` — the only way to discard —
is greppable and shows up in review as what it is. The usual failure mode of an
assertion inside a closure that never runs is not expressible.

**Assertions cannot escape into production code.** `Expect` is a capability
([`SPEC.md` §10](../SPEC.md)), granted only by the test platform. A library
function that wanted to assert would have to take `expect: Expect` in its
context, which its callers would have to hold, which only a test runner ever
does. `t.eq` in shipped code is a type error, not a code review finding.

## What a test can reach

A test source may import:

| | |
|---|---|
| The target under test | **by label**: `from "//lib/money" import { ... }` |
| The target's `deps` | The same libraries the target itself depends on |
| The suite's `test.deps` | Fakes, fixtures, matchers |
| `core/*` | Including `core/test` |

and may not:

- import relatively — `from "../cents" import { toCents }` is an error, and
  this is the rule that confines a test to the public surface;
- import another test source — test files are compiled independently and are
  not modules anybody can name. Shared helpers belong in a library listed in
  `test.deps`;
- be imported by anything;
- `export` anything. A test file's items are its `test` declarations and its
  private helpers.

```
error: lib/money/test/format.buri imports a library-internal module
  --> lib/money/test/format.buri:3:6
   |
 3 | from "../cents" import { toCents };
   |      ^^^^^^^^^^
   |
   = tests reach their library the same way dependents do
   = import //lib/money, and re-export `toCents` from lib/money/lib.buri if it
     is part of the surface you meant to test
```

That error is the whole design in one message. If a test needs an internal
function, either the function belongs on the surface — in which case say so in
`lib.buri` and everyone gets it — or the test is asserting on an implementation
detail and will break the next time the detail changes.

## Testing a binary

A binary's `main.buri` is its surface, exactly as `lib.buri` is a library's:

```buri
// cmd/server/main.buri

from "./routes" export { Route, route };

from "./routes" import { route };
from "core/cap" import { Alloc, Stdout, Net };
from "core/io" import * as io;

export fn main(ctx: { alloc: Alloc, stdout: Stdout, net: Net }): Result<{}, Str> {
  let _ = io.println(ctx, "listening");
  .Ok({})
}
```

```buri
// cmd/server/test/routes.buri

from "//cmd/server:server" import { route };
from "core/cap" import { Alloc };
from "core/test" import { Expect };
from "core/test" import * as t;

test "unknown paths route to the fallback" (ctx: { alloc: Alloc, expect: Expect }): Result<{}, Str> {
  t.eq(ctx, route("/nope").name, "fallback")?;
  .Ok({})
}
```

The binary is named with its full label because `//cmd/server` alone means the
package's library, and a binary package has none. Everything else is identical
to testing a library: the test sees what `main.buri` exports and nothing more,
so pushing logic behind the entry point is what makes it testable — which is
the pressure you want.

Testing `main` itself is possible and usually not what you want. It takes a
context; hand it fakes and check the `Result`:

```buri
from "//cmd/server:server" import { main };

test "main fails cleanly with no config" (ctx: { alloc: Alloc, expect: Expect }): Result<{}, Str> {
  // core/test is part of the test platform, so it is one of the few modules
  // that can hand out capability values — attenuated ones, in this case.
  let fake = { alloc: ctx.alloc, stdout: t.captureStdout(ctx), net: t.offlineNet(ctx) };
  let msg = t.isErr(ctx, main(fake))?;
  t.eq(ctx, msg.contains("config"), true)?;
  .Ok({})
}
```

## The test platform

The runner constructs the context each test declares, from a platform that
grants test implementations rather than OS ones. A test cannot ask for a
capability the test platform does not grant, and the test platform does not
grant real I/O:

| Capability | In a test |
|---|---|
| `Alloc` | Real, with a per-test arena the runner reclaims. |
| `Expect` | Real, and only here. |
| `Stdout`, `Stderr` | Captured. Printed only for a failing test. |
| `Fs` | In-memory, rooted at the package directory, containing exactly `test.data`. Writes are visible to that test and discarded after it. |
| `Net` | Refuses every connection. A fake goes in `test.deps`. |
| `Clock` | Starts at a fixed instant and advances only when the test advances it. |
| `Rand` | Seeded from the test's name, so a failure reproduces. |
| `Env` | Empty. |

This is defence in depth rather than the primary mechanism. The primary
mechanism is that a test which never asked for `Net` in its context type cannot
open a socket in any function it transitively calls — that is
[`SPEC.md` §10](../SPEC.md), not a build system feature. The sandbox in
[`HERMETICITY-AND-CACHING.md`](./HERMETICITY-AND-CACHING.md) is the third layer,
covering the compiler and the runner themselves.

`test { data: [...] }` declares the files the in-memory `Fs` contains:

```textproto
test {
  srcs: ["test/ledger.buri"]
  data: ["test/golden/statement.txt"]
  timeout_seconds: 30
}
```

```buri
from "core/fs" import * as fs;
from "core/cap" import { Alloc, Fs };

test "renders the statement" (ctx: { alloc: Alloc, expect: Expect, fs: Fs }): Result<{}, Str> {
  // `?` does not convert error types (SPEC.md 6.8), so the IoError goes
  // through `t.isOk`, which is the assertion that it succeeded at all.
  let want = t.isOk(ctx, fs.readText(ctx, "test/golden/statement.txt"))?;
  t.eq(ctx, render(ctx, sample()), want)?;
  .Ok({})
}
```

Rewriting a golden file is not something a hermetic action can do; see
[open question 5](./README.md#open-questions).

## Running

```
buri test //...                     every test in the repository
buri test //lib/money               one library's suite
buri test //lib/money --filter=pads substring match on test names
buri test //cmd/server:server       a binary's suite
buri test //... --output=js         run every suite through the JS backend
```

Output names the target, the file, and the test:

```
FAIL //lib/money  test/format.buri  "pads the cents place"
  expected: "$19.05"
  actual:   "$19.5"
  --> lib/money/test/format.buri:8:3

12 passed, 1 failed, 0 skipped (0.4s, 11 cached)
```

Tests are ordinary build actions: a suite whose sources, target, dependencies,
and toolchain are unchanged is not re-run, and reports as cached. Because there
is no mutable global state, no ambient I/O, and no observable ordering, the
runner is free to shard across processes and to run a suite's tests in any
order — `--shuffle` is on by default and the seed is printed.
