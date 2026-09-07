<!-- Edited by hand. The test suite in cli/tests/docs/ compiles every fenced
     example below, and runs the ones that pin output. -->

# Buri

A programming language for AI _and_ humans.

Buri is safe, fast to run, fast to compile, and friendly, in that order:

- **Safe**: a Buri program will not error. The language is purely functional, which rules out whole classes of logic bugs.
- **Fast to run**: Buri makes expensive actions explicit, and its syntax leaves a compiler plenty of room to optimize.
- **Fast to compile**: it typechecks a million lines a second and compiles tests 15 times faster than LLVM, on a build system built to scale to large repositories.
- **Friendly**: a clear type system, explicit syntax, and readable error messages. The linter pushes an AI to improve the architecture rather than patch the symptoms.

## What Buri is

A strict, purely functional, statically typed language that compiles to JavaScript and native code. Here's a sample program:

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
    // This context lets the program allocate and print to standard out, and
    // nothing else: no network calls, no filesystem.
    let ctx = context {
        Alloc: host.alloc,
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

## Finding your way

`buri docs` serves all four parts of the documentation from the binary.

- **[Getting started](cli/src/docs/getting-started/)** is the reading order for
  somebody new: why the language is shaped this way, how to install it, and one
  small program built end to end.
- **[Guides](cli/src/docs/guides/)** answer "how do I do X": set up an editor,
  write a test, compile to JavaScript. They also carry the few concepts you have
  to understand first, effects above all.
- **[The language](cli/src/docs/language/)** is the specification. Go there when
  you need the letter of the rule.
- **[Reference](cli/src/docs/reference/)** is lookup: the standard library, the
  build system, every CLI command, every error and lint code. You land on a page
  because something sent you there.

Getting started and the core guides are the required reading.

## Status

Buri is **version 0.3 and pre-release**: no tagged release, so every install
builds from source. Both backends, the build system, the test runner, the
formatter, the linter and the language server work end to end, and Buri builds
and tests itself. The surface is still moving, and this project will still make
a change that breaks your code.

## Installing

Every path below builds from source, and each produces the same binary.

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

`--HEAD` builds the `main` branch. You need it until a release is tagged.

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
system C toolchain (`cc`, or whatever `CC` names). The JavaScript path finds a
runtime — `bun` or `node` — on `PATH`, or wherever `BURI_JS` points.
