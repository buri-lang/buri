---
title: The network needs a networking-enabled toolchain
message: this toolchain was built without networking, so {operations} cannot be compiled
note: the native runtime carries the networking crates behind a `net` feature that is on by default, and this toolchain's copy of it was built without them
fix: install a toolchain built with the runtime's `net` feature, or build one with `cargo build -p buri` and `BURI_RUNTIME_NET` unset
reproduction: none
---
# The network needs a networking-enabled toolchain

```text
error: this toolchain was built without networking, so `host.HostListen.listen` cannot be compiled [networking-unavailable]
```

Your program is fine: a different toolchain compiles it unchanged.

`BURI_RUNTIME_NET=0` turns networking off. A build machine that couldn't reach
the runtime's dependencies also leaves it out, with a warning in the build log.
