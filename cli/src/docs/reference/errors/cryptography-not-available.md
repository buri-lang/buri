---
title: A program that needs unguessable randomness needs a toolchain built with cryptography
message: this toolchain was built without cryptography, so {operations} cannot be compiled
note: the native runtime reaches the operating system's generator through a `getrandom` dependency behind a `crypto` feature that is on by default, and this toolchain's copy of it was built without them
fix: install a toolchain built with the runtime's `crypto` feature, or build one with `cargo build -p buri` and `BURI_RUNTIME_CRYPTO` unset
reproduction: none
---
# A program that needs unguessable randomness needs a toolchain built with cryptography

```text
error: this toolchain was built without cryptography, so `host.HostEntropy.bytes` cannot be compiled [cryptography-not-available]
```

## What to do

Install or build a toolchain whose runtime archive can reach the operating
system's generator. The `crypto` feature is on by default, so an ordinary
`cargo build -p buri` produces one; `BURI_RUNTIME_CRYPTO=0` turns it off. A
machine that could not reach the runtime's dependencies at build time loses the
whole archive, with a warning in the build log.

Nothing about the program is wrong. A different toolchain compiles it unchanged.

## Why this is a refusal and not a fallback

`core/random` is always compiled in and would answer this call, but it promises
only that the output is uniform. `Entropy` promises that somebody who has
watched the output cannot predict the rest, and nothing tells the two apart by
inspection or by test. Substituting one for the other would be a security
failure with no symptom, so the compiler refuses instead.

If merely uniform octets are what you want, `core/random`'s `bytes` needs no
feature at all.
