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
- `main`'s body is the only place in a program where a context may be
  constructed (Section 11.3), and `core/host` is importable only by the module
  that exports `main` (Section 10.3). The context `main` builds is the program's
  complete effect budget.

`main` receives nothing and mints what it needs, so there is no fake to pass it
and nothing in it worth testing. Logic that wants a test goes in a function
`main` calls, which takes an ordinary bounded `ctx` and does not care where it
came from — the same pressure the build system applies to a binary's surface
([`cli/src/docs/build/testing.md`](./cli/src/docs/build/testing.md)).

### 11.1 Standard library conventions

Every function in the library sits in one of the three purity tiers of Section
10.5, and the tier is visible in the signature rather than in a comment: **pure**
takes no context parameter, **deterministic** takes one bounded by `Alloc` alone,
and **effectful** takes one bounded by anything else. What decides the tier is
Section 10.5's rule about size — an operation whose result size is fixed is pure,
and one whose result size depends on runtime data names `Alloc`. So `xs.len()`
and `s.trim()` are pure, `xs.map(ctx, f)` is deterministic, and
`fs.readText(ctx, p)` is effectful.

Two conventions run through the whole library. **Receiver first, context second**
(Section 10.7): everything that operates on a value is declared in an `impl`
block for that value's type and takes it as `self`, so it is callable as a
method — `xs.map(ctx, f)`, `s.trim()`, `opt.withDefault(0)` — with no import.
And **a name has one meaning**: there is no overloading, so a pure variant and an
allocating variant of the same idea get different names (`splitOnce` returns two
slices and is pure; `split` returns `[Str]` and allocates).

The catalogue itself is not normative in v0.3, and it is not here. Which modules
there are, what each one costs, and what is deliberately absent is
[`cli/src/docs/guide/standard-library.md`](./cli/src/docs/guide/standard-library.md),
and `buri docs core/list` renders a module from the source the compiler checked,
so a signature on that page is the signature that exists.

### 11.2 Tests

A **test source** is a module the build system compiles into a test binary rather
than into a library or a program; which modules are test sources is declared in a
build file ([`cli/src/docs/build/testing.md`](./cli/src/docs/build/testing.md)).
`test` declarations are legal there and nowhere else, and so are imports of
**test-only modules** — any module path containing a `testing` segment (Section
4.1.1). A test source may not `export`, and no module may import one: shared test
helpers are ordinary library code.

```buri repo=cli/tests/example role=test
from "//lib/money" import { fromCents };
from "core/testing/assert" import * as assert;
from "core/testing/context" import { Hermetic };

test "pads the cents place" {
  let ctx = Hermetic();
  assert.eq(fromCents(1905).format(ctx), "\$19.05");
}
```

A test declaration is `test STRING Block`. The name is a string literal because
test names are prose, and encoding prose in an identifier produces
`test_pads_the_cents_place` and then an argument about the convention. A test
takes no parameters and returns nothing: it passes unless an assertion in it
fails.

**A name is used once per file.** Two `test` declarations in one module with the
same name are a compile error (`duplicate-test-name`): a name is how a failing
test is reported and how `--filter` selects one, so two that share it in one
file cannot be told apart. Two *different* files may use the same name — they
are separate modules, and a report names the file each failure came from.

A test that needs a context builds one, with the same form `main` uses (Section
11.3). `core/testing/context` is a **platform module** — the test runner's
platform — and it exports one implementation per effect rather than one
pre-assembled world:

| Member | Effect | What it does |
|---|---|---|
| `alloc()` | `Alloc` | Real, from a per-test arena the runner reclaims. |
| `captureOut()`, `captureErr()` | `Stdout`, `Stderr` | Captured, and never printed; `captured()` is how a test reads it back. |
| `stdin([Str])` | `Stdin` | Reads the given lines, then end-of-input. |
| `data()` | `Fs` | In-memory, rooted at the package directory, containing exactly `test { data: [...] }`. |
| `files([(Str, Str)])` | `Fs` | In-memory, containing exactly these entries. |
| `readOnly(F)` | `Fs` | Wraps an `Fs` so every write fails. |
| `noNet()` | `Net` | Refuses every connection. |
| `clockAt(Int)` | `Clock` | Starts at that instant and advances only when the test advances it. |
| `randSeed(Int)` | `Rand` | Seeded, so a failure reproduces. |
| `envOf([(Str, Str)], [Str])` | `Env` | These variables and these arguments. |
| `Hermetic` | — | A context binding all of the above at hermetic defaults. |

Because a `testing` path may be imported only by a test source, nothing in a
shipped program can obtain any of them. And because effects are ordinary
interfaces (Section 10.9), a test needing behavior the runner does not provide
writes a struct with methods and binds that instead — there is no distinction
between the runner's implementations and yours.

#### 11.2.1 `core/testing/assert`

Assertions are an ordinary module, imported like any other. `assert` is not a
keyword: the name comes from `import * as assert`, and a file is free to call it
something else.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
from "core/testing/assert" import * as assert;
```

| Function | Meaning |
|---|---|
| `assert.eq(a, b)` | Fails unless `a == b`. Requires `Eq`, and `Show` for the message. |
| `assert.notEq(a, b)` | The negation. |
| `assert.isTrue(b)` / `assert.isFalse(b)` | On a `Bool`. |
| `assert.fail(msg)` | Fails unconditionally, with `msg`. |
| `assert.ok(r)` | Fails unless `r` is `.Ok`; **returns the wrapped value**. |
| `assert.err(r)` | Fails unless `r` is `.Err`; returns the error. |
| `assert.some(o)` | Fails unless `o` is `.Some`; returns the wrapped value. |

The first four return `()`; the last three return a value, and are how a
`Result` is consumed in a test, since `Result` is still must-use here:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
test "reads the config it wrote" {
  let ctx = Hermetic();
  assert.ok(fs.writeText(ctx, "cfg", "port=8080"));   // returns (), so a statement
  let text = assert.ok(fs.readText(ctx, "cfg"));      // returns Str, so a binding
  assert.eq(text, "port=8080");
}
```

Two things about `core/testing/assert` are not ordinary, and both follow from
its being a platform module rather than a library:

- **Its functions take no `ctx`** and still render a failure message. Rendering
  is the runner's, not the program's — which is why this signature would be a
  lie anywhere else, and why the module is importable only from a test source.
- **A failure ends that test** and no other, the way an abort (Section 6.10)
  ends a program. The runner reports the file, the line, and both values.

A test source may also use **expression statements**, which no other module may:
*any* expression whose type is `()` may stand alone, terminated by `;`. A call
is the common case, and a `match`, an `if` or a block whose every branch
produces `()` is one too.

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
assert.eq(total, 42);              // statement: type is ()
match (parsed) {                   // statement: every arm is ()
  .Some(n) => assert.eq(n, 42),
  .None => assert.fail("no value"),
};                                 // ← the `;` is what makes it a statement
// assert.ok(loadConfig(ctx));     // ERROR if it returns Config — bind it or drop
                                   // it explicitly with `let _ =`
```

This is the narrowest relaxation that makes assertions read as assertions, and
it does not weaken Section 5.7.1: `Result` is not `()`, so nothing must-use can
be dropped by it.

The `;` is not decoration, and a `{`-initial expression carries it like any
other. A block is statements followed by a result expression, and the `;` is
the only thing that says which one this is; without it a `match` in the middle
of a test body reads as the block's result, and what follows has nowhere to go
(Section 12.2).

### 11.3 Contexts

A context is built by naming each effect it provides and the value that
implements it. There is one form, and `main` and a test use the same one.

**As an expression**, anonymous:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Fs, Stdout };
let ctx = context {
  Alloc:  host.alloc,
  Stdout: host.stdout,
  Fs:     rooted(host.fs, "/srv/app"),
};
```

**As a declaration**, named — so a fixture can be shared by every test in a file,
or exported from a test-only module and shared across files:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Alloc, Clock, Env, Fs, Net, Rand, Stderr, Stdout };
context Hermetic {
  Alloc:  alloc(),
  Stdout: captureOut(),
  Stderr: captureErr(),
  Fs:     data(),
  Net:    noNet(),
  Clock:  clockAt(0),
  Rand:   randSeed(0),
  Env:    envOf([], []),
}
```

A named context is **constructed by calling it** — `Hermetic()` — and each call
builds a fresh one. The parentheses are not decoration: a test's `Fs` and its
captured `Stdout` accumulate what the test does to them, so two tests sharing
one value would share its state. A context declaration takes no parameters; what
varies between call sites is expressed by overriding, not by arguments.

**Either form may begin with a spread**, which takes every binding from another
context and lets the ones that follow replace them:

```buri ignore why="not yet converted to a compiled example: it references names the document never declares, so it needs a preamble before the harness can check it"
# from "core/effect" import { Fs };
context Fixture {
  ..Hermetic(),
  Fs: files([("config.toml", "port=8080")]),
}

test "rejects a port above 65535" {
  let ctx = context { ..Fixture(), Fs: files([("config.toml", "port=99999")]) };
  let e = assert.err(loadConfig(ctx, "config.toml"));
  assert.eq(e, ConfigError.PortOutOfRange);
}
```

So a file may declare one context for all of its tests, a test may build its
own, and either may start from another and change one line.

**Where a context may be built:**

| | `context` declaration | Constructing one |
|---|---|---|
| The module exporting `main` | yes | only inside `main`'s body |
| A test source | yes | anywhere in the file |
| A test-only module (a `testing` path segment) | yes, and may be exported | anywhere in the file |
| Anywhere else | no | no |

That table is the whole restriction, and between it and `core/host`'s import
rule (Section 4.1.1) it is the reason the purity theorem's last clause is
vacuous in ordinary code. Neither a `context` expression nor a call to a named
context may appear inside a lambda, even where both are otherwise legal; without
that, a closure could mint authority and Section 10.6 would not mean what it
says.

**What is checked:**

- Every binding's left side names a declared effect, and no effect is bound
  twice — counting a spread, whose bindings an explicit one replaces rather than
  duplicates.
- Every binding's right side is a value whose type implements that effect
  (ordinary nominal conformance, Section 5.12.1).
- The constructed value satisfies exactly the effects bound and nothing else, so
  it is accepted by any `<C: ...>` naming a subset of them and rejected by any
  naming more.

A context's type is generated, has no name, and is never written down. Contexts
flow only into `ctx` parameters, which are bounded by effects rather than typed
by a context, so there is nothing to spell — which is why this does not
reintroduce the structural records of Section 5.5.

The bindings use `:` rather than `=` for the same reason struct literals do: a
brace-delimited list of `Name: value` pairs is a shape the language already has.
What differs is that the name on the left is an effect rather than a field,
which is visible in its case.

---
