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
system's generator. The runtime's `crypto` feature is on by default, so an
ordinary `cargo build -p buri` produces one; `BURI_RUNTIME_CRYPTO=0` is what
turns it off, and a machine that could not reach the runtime's dependencies when
the toolchain was built loses the whole archive with a warning in the build log.

Nothing about the program is wrong, and nothing in it needs editing. A different
toolchain compiles it unchanged.

## Why this is a refusal and not a fallback

There is another generator in the same archive. `core/random` is backed by
xoshiro256++ over a seed read once, it is a few hundred bytes of code, it is
always compiled in, and it would answer this call without complaint.

It must not, and that is the whole reason this page exists. `Entropy` promises
that somebody who has watched the output cannot predict the rest, and `Rand`
promises only that the output is uniform. Octets from the two are
indistinguishable by inspection, by test, and by every part of a program except
the attacker it was defending against — so a substitution here would be a
security failure with no symptom, discovered by the person it was made against.

A refusal names the operation, at compile time, to somebody who can still do
something about it. `core/random`'s `bytes` is the door for octets that are
merely uniform, and it needs no feature at all.

## Why it is not a link error

The alternative is an unresolved `buri_rt_host_entropy_bytes` from the system
linker: a mangled symbol, in a message about an archive, handed to somebody who
wrote a program. The refusal is made before code generation, from the feature
list the build script wrote beside the archive it built.
