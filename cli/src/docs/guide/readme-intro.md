## What Buri is

Buri is a strict, purely functional, statically typed language and the
single-binary toolchain that implements it. There is no `null`, no exception,
no mutation and no aliasing; `match` is exhaustive, indexing answers `Option`,
and a `Result` cannot be dropped on the floor. Anything a function can do
besides compute — allocate, print, read a file, open a socket — has to arrive
as an effect value in a parameter the compiler checks, so a signature says what
a function is allowed to do and not only what it takes.

```buri run
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

enum Grade {
  Pass(Int),
  Fail { score: Int, needed: Int },
}

impl Grade {
  // No context parameter, so this cannot allocate, print, read a file, or open
  // a socket. That is in the signature, not in this comment, and the compiler
  // is what holds it.
  fn shortfall(self: Grade): Int {
    match (self) {
      .Pass(_) => 0,
      .Fail { score, needed } => needed - score,
    }
  }
}

// `main` takes no arguments. It builds the one context the program has, and
// those two bindings are the whole effect budget: this program can allocate and
// it can print, and there is nothing else it could reach for.
export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
  };

  let grades = [Grade.Pass(91), Grade.Fail { score: 48, needed: 60 }];
  let missed = grades.map(ctx, fn(g) => g.shortfall()).sum();
  let _ = ctx.println("points short: ${missed}");
  .Ok(())
}
```

```stdout
points short: 12
```

Three goals order every trade in the design: **safe, fast to run, fast to
compile** — in that order when they conflict
([`guide/goals.md`](./cli/src/docs/guide/goals.md) has what each bought and
cost). The toolchain is one binary with nothing to configure: the build system,
the test runner, the formatter, the linter, the language server, and the
documentation — every example of which is compiled by the test suite.

## Status

Buri is **version 0.3 and pre-release**: no tagged release, every install
builds from source. What is here works end to end — both backends (native and
JavaScript), the build system, the test runner, the formatter, the linter, the
language server — and it builds and tests itself. The surface is still moving,
and a change that breaks your code is a change this project will still make.
