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
produces one. `BURI_RUNTIME_NET=0` turns it off, and a machine that could not
reach the runtime's dependencies when the toolchain was built gets the same
result plus a warning in the build log.

## Why

A runtime without networking is a missing *language capability*, so the compiler
says so before code generation rather than leaving the linker to report an
unresolved `buri_rt_*` symbol. Nothing about your program is wrong: a different
toolchain compiles it unchanged.
