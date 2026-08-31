<!-- Edited by hand. Every fenced example below is compiled — and run, where it
     pins output — by the test suite in cli/tests/docs/. -->

# Buri

A programming language for AI _and_ humans.

Buri is safe, fast to run, fast to compile, and friendly - in that order:

- **Safe**: Buri will not error, is functional which rules out entire classes of logic bugs, and Buri code is easy to verify it works as expected whether you wrote the code or not.
- **Fast to run**: make expensive actions explicit, choose syntax that enables compilers to optimize the code a lot.
- **Fast to compile**: typechecks 1 million lines of code per second, and compiles tests 15 times faster than LLVM, with a build system designed to scale large repositories.
- **Friendly**: clear type system, explicit syntax, readable error messages, linter messages encourages AI to improve code architecture not just "fix the symtoms".

## What Buri is

Buri is a strict, purely functional, statically typed language that compiles to JavaScript and native code. Here's a sample program:

```buri run
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

enum Grade {
  Pass(Int),
  Fail { score: Int, needed: Int },
}

impl Grade {
  // No ctx parameter, so this cannot allocate, print, read a file, or open
  // a socket.
  fn shortfall(self): Int {
    match (self) {
      .Pass(_) => 0,
      .Fail { score, needed } => needed - score,
    }
  }
}

// `main` is the entry point and builds the `context`.
export fn main(): Result<(), Str> {
// This contxt declares the program can allocate to the heap
// and print to standard out, and nothing else
// (no network calls, no file system operations, etc.).
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
  };

  let grades = [Grade.Pass(91), Grade.Fail { score: 48, needed: 60 }];
  let missed = grades.map(ctx, fn(g) => g.shortfall()).sum();
  let _ = io.println(ctx, "points short: ${missed}").ignore();
  .Ok(())
}
```

```stdout
points short: 12
```

## Status

Buri is **version 0.3 and pre-release**: no tagged release, every install
builds from source. What is here works end to end — both backends (native and
JavaScript), the build system, the test runner, the formatter, the linter, the
language server — and it builds and tests itself. The surface is still moving,
and a change that breaks your code is a change this project will still make.

## Installing

There is no release yet, so every path below builds from source. They produce
the same binary and differ only in what supplies the Rust toolchain.

**Nix.** This repository is a flake, and its default package is `buri`:

```sh
nix run github:buri-lang/buri -- version   # run it once, install nothing
nix profile install github:buri-lang/buri  # keep it
```

**Homebrew.** This repository is also its own tap:

```sh
brew tap buri-lang/buri https://github.com/buri-lang/buri.git
brew install --HEAD buri-lang/buri/buri
```

`--HEAD` builds the `main` branch and is required until a release is tagged;
after that, drop it.

**Cargo**, with a Rust toolchain already in hand:

```sh
cargo install --locked --path cli
```

Your first repository is one command. `buri init` writes a working library, a
binary that depends on it, and a test suite, and installs the agent skills:

```sh
buri init hello-buri && cd hello-buri && buri test //...
```

The binary carries no runtime dependencies. Linking a native binary uses the
system C toolchain (`cc`, or whatever `CC` names); the JavaScript path resolves
a runtime — `bun` or `node` — from `PATH`, or from `BURI_JS` naming one.
