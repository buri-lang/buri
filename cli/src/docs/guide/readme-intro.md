## What Buri is

Buri is a strict, purely functional, statically typed language and the
single-binary toolchain that implements it. There is no `null`, no exception,
no mutation and no aliasing; `match` is exhaustive, indexing answers `Option`,
and a `Result` cannot be dropped on the floor. Anything a function can do
besides compute — allocate, print, read a file, open a socket — has to arrive
as an effect value in a parameter the compiler checks, so a signature says what
a function is allowed to do and not only what it takes.

Three goals order every trade in the design: **safe, fast to run, fast to
compile** — in that order when they conflict, and secondarily one language that
targets both a native binary and JavaScript. That is why generics are
monomorphized rather than dictionary-passed, why tail calls are guaranteed and
lowered to loops where the host lacks them, why top-level signatures are
mandatory so each function body checks on its own, and why the grammar is
unambiguous with no feedback from name resolution into the parser.

The toolchain is one binary with no dependencies and nothing to configure. It
holds the build system, the test runner, the formatter, the linter, the
language server, the protobuf schema compiler, and the documentation you are
reading. Every example in that documentation is compiled by the test suite, and
the ones that print something are run and their output compared — so the
documentation cannot drift away from the language.
