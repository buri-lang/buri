---
title: Secure randomness needs a cryptography-enabled toolchain
message: this toolchain was built without cryptography, so {operations} cannot be compiled
note: the native runtime reaches the operating system's generator through a `getrandom` dependency behind a `crypto` feature that is on by default, and this toolchain's copy of it was built without them
fix: install a toolchain built with the runtime's `crypto` feature, or build one with `cargo build -p buri` and `BURI_RUNTIME_CRYPTO` unset
reproduction: none
---
# Secure randomness needs a cryptography-enabled toolchain

```text
error: this toolchain was built without cryptography, so `host.HostEntropy.bytes` cannot be compiled [cryptography-unavailable]
```

The program is fine; a different toolchain compiles it unchanged. The `crypto`
feature is on by default, so a plain `cargo build -p buri` has it, and
`BURI_RUNTIME_CRYPTO=0` turns it off. A machine that couldn't fetch the
runtime's dependencies at build time loses the whole runtime archive, with a
warning in the build log.

The same feature carries `ring`, so `core/crypto`'s `seal`, `open`,
`verifyEs256`, `verifyRs256` and `verifyEd25519` are refused the same way.

There's no fallback to `core/random`. It promises only uniform output, while
`Entropy` promises that watching the output can't predict the rest. No test
tells them apart, so swapping one for the other would be a silent security
failure. If uniform bytes are all you need, `core/random`'s `bytes` needs no
feature.
