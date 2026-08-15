## Goals

**Safe, fast to run, fast to compile** — in that order when they conflict.
Secondarily, one language that targets both a native binary and JavaScript.

Those goals are why the design looks the way it does:

| Goal | What it bought, and what it cost |
|---|---|
| **Safe** | No `null`, no exceptions, no mutation, no aliasing. Exhaustive `match`. Indexing returns `Option`. `Result` is must-use. Effects require an effect value you were handed, in a parameter position the compiler enforces. Out-of-range numeric literals are compile errors. |
| **Fast to run** | Strict evaluation with fully specified order — no thunks, no space leaks. Monomorphized generics, no dictionaries. Guaranteed tail calls, lowered to loops where the host lacks them. Immutability lets the runtime reuse memory in place when a value is provably unshared. `Alloc` as an effect makes allocation visible at every call site that does it. |
| **Fast to compile** | An unambiguous LR(1) grammar with no name-resolution or type feedback into the parser, so parsing is one pass and trivially parallel across files. Mandatory top-level signatures make type inference local to each function body, so modules check independently and incrementally. No macros, no reflection, no overload resolution, no row unification. Conformance is nominal and declared in one module, so there is no coherence pass and no instance search — a bound is a table lookup. The one concession: method resolution needs the receiver's type, so name resolution and inference interleave. |
| **Binary and JS** | Nothing in the semantics assumes a machine word: `Int` is `I64` everywhere, integer overflow is undefined rather than quietly wrapping, and evaluation order is specified rather than left to the backend. The effect model maps onto a browser platform as cleanly as onto a POSIX one — a JS target simply exports a different `core/host`. |

Where they pull against each other, the compiler absorbs it rather than the
language: guaranteed tail calls become loops on a JS target, since no engine but
JavaScriptCore implements them natively ([SPEC.md §8.3.1](./SPEC.md)). `I64` on
JS is the one genuinely unresolved tension — see [SPEC.md §15](./SPEC.md).

[SPEC.md §13](./SPEC.md) states the invariants that make the compile-speed goal
reachable, so a future feature can be measured against them rather than
quietly eroding them.

**This repository is the specification and the toolchain that implements it.**
Every example below is compiled by `cargo test`, and the ones that print
something are run and their output compared — so the documentation cannot drift
from the language. `buri docs` serves all of it from the binary.

- [`SPEC.md`](./SPEC.md) — the language reference
- [`grammar.ebnf`](./cli/src/docs/grammar.ebnf) — the normative grammar, in extended BNF
- [`cli/src/docs/`](./cli/src/docs/) — the monorepo build system: `BUILD.buri`
  files, library and binary targets, visibility, tags, hermetic incremental
  builds, and one CLI. [`cli/tests/example/`](./cli/tests/example/) is a
  worked monorepo, and the largest body of Buri here to read.
- [`cli/tests/`](./cli/tests/) — how the toolchain is held to all of the above

```buri run
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

enum Shape {
  Circle(Float),
  Rect { width: Float, height: Float },
  Empty,
}

impl Shape {
  // No context parameter, so this cannot allocate, print, read a file, or open
  // a socket. It is a mathematical function of its argument, and you can see
  // that from the signature alone.
  fn area(self: Shape): Float {
    match (self) {
      .Circle(r) => 3.14159 * r * r,
      .Rect { width, height } => width * height,
      .Empty => 0.0,
    }
  }
}

// `main` takes no arguments. It builds the one context the program has, and
// those two bindings are the whole effect budget.
export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
  };

  let shapes = [Shape.Circle(1.0), Shape.Rect { width: 2.0, height: 3.0 }];
  let total = shapes.map(ctx, fn(s) => s.area()).sumFloat();
  let _ = ctx.println("total area: ${total}");
  .Ok(())
}
```

```stdout
total area: 9.14159
```
