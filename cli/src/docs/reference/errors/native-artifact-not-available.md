---
title: A native artifact is built where this toolchain can build one
message: no native artifact for {output}
note: {reason}
fix: {fix}
reproduction: none
---
# A native artifact is built where this toolchain can build one

The note says which of three things is missing, and each has its own answer.

## The host

A native artifact is built on the machine it runs on. The runtime archive is
built for the host by `cli/build.rs`, the C library it is finished with is the
host's, and the linker is the host's — so an output is built on a machine of the
platform and the architecture it names. Build it there, or declare a `JS` output
and run the module anywhere.

## The runtime archive

A toolchain built on a host the runtime is not built for carries no
`libburi_rt.a` at all, and a native artifact would have nothing to link against.
Every output such a toolchain can produce is a JavaScript module.

## The backend

`--release` is the *optimizing* native code generator, which arrives with the
`backend-llvm` cargo feature and is off by default: it needs LLVM 21 installed
and `LLVM_SYS_211_PREFIX` set, and `cargo install buri` must not require either.
A toolchain without it builds the same output in the development profile — the
same command with no `--release` — through the copy-and-patch backend, which is
compiled in by default.

## Why

A `--release` build is **not** quietly handed to the development backend when
the optimizing one is absent. `--release` producing different code depending on
how the compiler happened to be installed is the same class of bug as an
unpinned toolchain: two machines would ship two artifacts from one source and
one commit, and neither could be reproduced from the other.

So the answer is a refusal, and what this page is really about is that the
refusal has to be *true*. One sentence used to serve all three causes — "the
backend is not implemented", with a fix reading "this toolchain emits
JavaScript" — and it was false on two of them: false on a host that had just
built the very same output for the very same platform one line earlier, and
false on any toolchain that emits a native executable at all. A refusal that
names the wrong cause costs more than no refusal, because it sends the reader to
change something that was never wrong.
