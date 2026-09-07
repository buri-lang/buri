## 11. Programs

A program is a module that exports `main`:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Env, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
    Env:    host.env,
  };
  ...
}
```

- `main` must take no parameters and declare no generic parameters.
- `main` must return `Result<(), Str>`.
- `.Ok(())` exits 0. `.Err(msg)` prints `msg` to stderr and exits 1.
- An **entry's** body is the only place in a program that may construct a context
  (Section 11.3), and only the module exporting it may import `core/host`
  (Section 10.3). The context an entry builds is that artifact's complete effect
  budget.

An entry is a function a build file's `outputs` names, and `main` is the one it
names by default. A binary may declare several — a page entering at `main` and a
worker at `fetch`, out of one module — and each platform fixes the signature of
the entry it calls
([`cli/src/docs/reference/build/build-files.md`](./cli/src/docs/reference/build/build-files.md)).

An entry receives nothing it can be handed a double for, so there is no fake to
pass it and nothing in it worth testing. Put logic you want to test in a function
it calls, taking an ordinary bounded `ctx`
([`cli/src/docs/reference/build/testing.md`](./cli/src/docs/reference/build/testing.md)).

### 11.1 Standard library conventions

Every function in the library sits in one of the three purity tiers of Section
10.5, and the signature shows which. **Pure** takes no context parameter,
**deterministic** takes one bounded by `Alloc` alone, and **effectful** takes one
bounded by anything else. Size decides: an operation whose result size is fixed
is pure, and one whose result size depends on runtime data names `Alloc`. So
`xs.len()` and `s.trim()` are pure, `xs.map(ctx, f)` is deterministic, and
`fs.readText(ctx, p)` is effectful.

Two conventions run through the whole library. **Receiver first, context second**
(Section 10.7): everything that operates on a value lives in an `impl` block for
that value's type and takes it as `self`, so you call it as a method —
`xs.map(ctx, f)`, `s.trim()`, `opt.withDefault(0)` — with no import. And **a name
has one meaning**: there is no overloading, so a pure variant and an allocating
variant of the same idea get different names. `splitOnce` returns two slices and
is pure; `split` returns `[Str]` and allocates.

The catalogue is not normative in v0.3 and is not here.
[`cli/src/docs/reference/standard-library.md`](./cli/src/docs/reference/standard-library.md)
lists which modules there are, what each one costs, and what is deliberately
absent. `buri docs core/list` renders a module from the source the compiler
checked.

### 11.2 Tests

A **test source** is a module the build system compiles into a test binary rather
than into a library or a program. A build file declares which modules are test
sources ([`cli/src/docs/reference/build/testing.md`](./cli/src/docs/reference/build/testing.md)).
`test` declarations are legal there and nowhere else, and so are imports of
**test-only modules** — any module path containing a `testing` segment (Section
4.1.1). A test source may not `export`, and no module may import one, so shared
test helpers are ordinary library code.

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

A test declaration is `test STRING Block`. A test takes no parameters and returns
nothing: it passes unless an assertion in it fails.

**A name is used once per file.** Two `test` declarations in one module with the
same name are a compile error (`duplicate-test-name`), since the name is how a
report identifies a failing test and how `--filter` selects one. Two *different*
files may use the same name.

A test that needs a context builds one, with the same form `main` uses (Section
11.3). `core/host/testing` is a **platform module**, the test runner's platform:
`core/host`'s surface written out for a test, with the same names **called**
rather than referred to, so each call answers a fresh double.

| Member | Effect | What it does |
|---|---|---|
| `alloc()` | `Alloc` | Real, from a per-test arena the runner reclaims. |
| `stdout()`, `stderr()` | `Stdout`, `Stderr` | Captured, and never printed; `captured()` is how a test reads it back. |
| `stdin()` | `Stdin` | At end of input, so a suite never blocks on a pipe nobody is writing to. |
| `fs()` | `FsRead`, `FsWrite` | In-memory and empty. Writes are visible to that test and discarded after it. |
| `net()` | `Net` | Refuses every request until `respond` says what to answer. |
| `clock()` | `Clock` | At zero, and advances only when the test advances it. |
| `rand()` | `Rand` | Seeded at zero, so a failure reproduces. |
| `env()` | `Env` | No variables and no arguments. |
| `proc()` | `Proc` | Absorbs the exit instead of taking it, so the test carries on. |
| `tasks()` | `Tasks` | Runs the tasks one at a time, in program order. |

You configure a double with a **method that answers a new handle**, rather than
with an argument to the constructor — `clock().at(1000)`, `rand().seed(7)`,
`env().variables([...]).arguments([...])`, `stdin().lines([...])`,
`fs().files([...])`, `fs().readOnly()`, `net().respond(...)` — so a chain reads
in the order it applies, and the value it was called on does not change.

`fs()` is one double answering **two** effects, so a context that reads and
writes binds the one value under both names — `let disk = fs(); ... FsRead:
disk, FsWrite: disk` — and two calls to `fs()` are two filesystems that share
nothing.

Only a test source may import a `testing` path, so nothing in a shipped program
can obtain any of them. Effects are ordinary interfaces (Section 10.9), so a test
needing behavior the runner does not provide writes a struct with methods and
binds that instead.

#### 11.2.1 `core/testing/assert`

Assertions are an ordinary module, imported like any other. `assert` is not a
keyword: the name comes from `import * as assert`, and a file may call it
something else.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
from "core/testing/assert" import * as assert;
```

| Function | Meaning |
|---|---|
| `assert.eq(a, b)` | Fails unless `a == b`. Requires `Eq`, and `Show` for the message. |
| `assert.notEq(a, b)` | The negation. |
| `assert.isTrue(b)` / `assert.isFalse(b)` | On a `Bool`. |
| `assert.contains(xs, x)` | Fails unless `x` is an element of `xs`. |
| `assert.isEmpty(xs)` / `assert.notEmpty(xs)` | On a list. |
| `assert.len(xs, n)` | Fails unless `xs` holds exactly `n` elements. |
| `assert.gt(a, b)` / `ge` / `lt` / `le` | The comparisons, on an `Ord`. |
| `assert.approxEq(a, b, tolerance)` | On `Float`, within an absolute tolerance. |
| `assert.ok(r)` | Fails unless `r` is `.Ok`; **returns the wrapped value**. |
| `assert.err(r)` | Fails unless `r` is `.Err`; returns the error. |
| `assert.some(o)` | Fails unless `o` is `.Some`; returns the wrapped value. |

There are so many of them because of the message: each one names the two values
it compared, where `assert.isTrue(xs.contains(x))` says only "expected true, got
false". There is **no `assert.fail`**; a test that has to fail asserts on the
value it has.

Everything above the last three returns `()`. Those three return a value, and are
how a test consumes a `Result`, which is still must-use here:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
test "reads the config it wrote" {
    let disk = fs();
    let ctx = context {
        Alloc: alloc(),
        FsRead: disk,
        FsWrite: disk,
    };
    let cfg = path.of(ctx, "cfg");
    assert.ok(fs.writeText(ctx, cfg, "port=8080")); // returns (), so a statement
    let text = assert.ok(fs.readText(ctx, cfg)); // returns Str, so a binding
    assert.eq(text, "port=8080");
}
```

Two things about `core/testing/assert` are not ordinary, and both follow from it
being a platform module rather than a library:

- **Its functions take no `ctx`** and still render a failure message. The runner
  does that rendering, not the program. That is why this signature would be a lie
  anywhere else, and why only a test source may import the module.
- **A failure ends that test** and no other, the way an abort (Section 6.9)
  ends a program. The runner reports the file, the line, and both values.

A test source may also use **expression statements**, which no other module may:
*any* expression whose type is `()` may stand alone, terminated by `;`. A call is
the common case; a `match`, an `if` or a block whose every branch produces `()`
counts too.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
assert.eq(total, 42);              // statement: type is ()
match (parsed) {                   // statement: every arm is ()
  .Some(n) => assert.eq(n, 42),
  .None => assert.eq(parsed, .Some(42)),
};                                 // ← the `;` is what makes it a statement
// assert.ok(loadConfig(ctx));     // ERROR if it returns Config — bind it or drop
                                   // it explicitly with `let _ =`
```

This does not weaken Section 5.7.1: `Result` is not `()`, so it can drop nothing
must-use.

The `;` is not decoration, and a `{`-initial expression carries it like any
other. Without it, a `match` in the middle of a test body reads as the block's
result, and what follows has nowhere to go (`design/grammar-rationale.md` 12.2).

### 11.3 Contexts

You build a context by naming each effect it provides and the value that
implements it. `main` and a test use the same form.

**As an expression**, anonymous:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Stdout };
# from "core/fs" import { FsRead };
let ctx = context {
  Alloc:  host.alloc,
  Stdout: host.stdout,
  FsRead: rooted(host.fs, "/srv/app"),
};
```

**As a declaration**, named — so a fixture can be shared by every test in a file,
or exported from a test-only module and shared across files:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Clock, Env, Net, Rand, Stderr, Stdout };
# from "core/fs" import { FsRead };

context Sandbox {
    Alloc: alloc(),
    Stdout: stdout(),
    Stderr: stderr(),
    FsRead: fs(),
    Net: net(),
    Clock: clock(),
    Rand: rand(),
    Env: env(),
}
```

You **construct a named context by calling it** — `Sandbox()` — and each call
builds a fresh one. The parentheses are not decoration: a test's filesystem and
its captured `Stdout` accumulate what the test does to them, so two tests sharing
one value would share its state. That is also why `Sandbox` binds `FsRead` and
not `FsWrite`: each binding is its own expression, so a declaration naming both
halves would call `fs()` twice and hand the test two unrelated filesystems. A
test that writes and reads back binds one `fs()` to both names in a `context`
**expression**, where a `let` can hold it. A context declaration takes no
parameters; override a binding to vary what a call site gets.

**Either form may begin with a spread**, which takes every binding from another
context and lets the ones that follow replace them:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/fs" import { FsRead };

context Fixture {
    ..Sandbox(),
    FsRead: fs().files([("config.toml", "port=8080")]),
}

test "rejects a port above 65535" {
    let ctx = context {
        ..Fixture(),
        FsRead: fs().files([("config.toml", "port=99999")]),
    };
    let e = assert.err(loadConfig(ctx, "config.toml"));
    assert.eq(e, ConfigError.PortOutOfRange);
}
```

So a file may declare one context for all of its tests, a test may build its
own, and either may start from another and change one line.

**Where a context may be built:**

| | `context` declaration | Constructing one |
|---|---|---|
| The module exporting `main` | yes | only inside an entry's body |
| A test source | yes | anywhere in the file |
| A test-only module (a `testing` path segment) | yes, and may be exported | anywhere in the file |
| Anywhere else | no | no |

That table is the whole restriction, and together with `core/host`'s import rule
(Section 4.1.1) it is why the purity theorem's last clause is vacuous in ordinary
code. Neither a `context` expression nor a call to a named context may appear
inside a lambda, even where both are otherwise legal. Without that, a closure
could mint authority and Section 10.6 would not mean what it says.

**What the compiler checks:**

- Every binding's left side names a declared effect, and no effect is bound
  twice — counting a spread, whose bindings an explicit one replaces rather than
  duplicates.
- Every binding's right side is a value whose type implements that effect
  (ordinary nominal conformance, Section 5.12.1).
- The constructed value satisfies exactly the effects bound and nothing else, so
  any `<C: ...>` naming a subset of them accepts it and any naming more rejects
  it.

The compiler generates a context's type. It has no name, and nobody writes it
down. Contexts flow only into `ctx` parameters, and effects bound those
parameters rather than a context typing them, so there is nothing to spell. That
is why this does not reintroduce the structural records of Section 5.5.

---
