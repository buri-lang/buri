---
title: A native artifact is built where this toolchain can build one
message: no native artifact for {output}
note: {reason}
fix: {fix}
reproduction: none
---
# A native artifact is built where this toolchain can build one

The note names which of three things is missing. Each has its own answer.

## The host

A native artifact only builds on a machine of the platform and architecture it
names: the runtime archive, the C library it links against and the linker are
all the host's. Build the output on such a machine, or declare a `JS` output and
run the module anywhere.

## The runtime archive

A toolchain built on a host the runtime does not cover carries no
`libburi_rt.a`, so a native artifact would have nothing to link against. Every
output such a toolchain can produce is a JavaScript module.

## The backend

`--release` uses the *optimizing* native code generator. It arrives with the
`backend-llvm` cargo feature and is off by default: it needs LLVM 21 installed
and `LLVM_SYS_211_PREFIX` set. Drop `--release` and the copy-and-patch backend,
which is compiled in by default, builds the same output.

## Why

The compiler will **not** quietly hand a `--release` build to the development
backend when the optimizing one is absent. If `--release` produced different
code depending on how you installed the compiler, two machines would ship two
artifacts from one source and one commit, and neither could be reproduced from
the other.
