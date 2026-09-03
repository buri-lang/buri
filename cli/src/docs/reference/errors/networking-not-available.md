---
title: A program that uses the network needs a toolchain built with networking
message: this toolchain was built without networking, so {operations} cannot be compiled
note: the native runtime carries the networking crates behind a `net` feature that is on by default, and this toolchain's copy of it was built without them
fix: install a toolchain built with the runtime's `net` feature, or build one with `cargo build -p buri` and `BURI_RUNTIME_NET` unset
reproduction: none
---
# A program that uses the network needs a toolchain built with networking

```text
error: this toolchain was built without networking, so `host.HostListen.listen` cannot be compiled [networking-not-available]
```

## What to do

Install or build a toolchain whose runtime archive has networking in it. The
runtime's `net` feature is on by default, so an ordinary `cargo build -p buri`
produces one; `BURI_RUNTIME_NET=0` is what turns it off, and a machine that
could not reach the runtime's dependencies when the toolchain was built gets the
same result with a warning in the build log.

## Why

The compiler and the runtime are admitted through different doors. A toolchain
without a release code generator is a toolchain a contributor can still work
with; a runtime without networking is a *language capability* missing, and the
program that reaches for it has to be told so in those words.

It is told before code generation rather than at link time. The alternative is
an unresolved `buri_rt_*` symbol from the system linker, naming a mangled symbol
in a message about an archive — a sentence about this repository's internals
handed to somebody who wrote a program.

Nothing about the program is wrong, and nothing in it needs editing. A different
toolchain compiles it unchanged.
