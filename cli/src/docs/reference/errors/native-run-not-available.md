---
title: A suite that names no platform runs natively, so this toolchain has to be able to build one
message: this toolchain cannot build a {platform} test binary in the {profile} profile
note: a native run needs a code generator for this profile compiled into this toolchain, a runtime archive for this host, and a C toolchain to link them with
fix: run the suite on JavaScript with `buri test --output=js`, or declare `test {{ platforms: [JS] }}` if that is where it belongs
reproduction: none
---
# A suite that names no platform runs natively, so this toolchain has to be able to build one

```text
error: this toolchain cannot build a macos test binary in the debug profile [native-run-not-available]
```

## What to do

Either give this invocation a backend it can use, or say out loud that the suite
runs on JavaScript.

`buri test --output=js` says it for one invocation and changes nothing in the
repository. `test { platforms: [JS] }` says it for the suite, which is the right
answer when the suite belongs there for good.

The other direction is to fix the toolchain. The two profiles need different
things:

- **debug** wants the `backend-stencil` feature (on by default), a stencil
  library for this host's triple, the runtime archive `cargo build -p buri`
  compiles, and a C compiler on `PATH` — `cc`, or whatever `CC` names.
- **release** wants `backend-llvm`, which is off by default and needs LLVM 21
  installed with `LLVM_SYS_211_PREFIX` set. A toolchain without it refuses
  rather than quietly handing the release build to the development backend.

## Why

`buri test` runs a suite that names no platform on the host, natively. Nothing
about your program is wrong; what is missing is a piece of this toolchain, and
both escape hatches above are you saying where the suite runs instead.

A platform a suite *asked* for is refused separately, with
[`platform-not-implemented`](platform-not-implemented.md). A suite naming
`platforms` has somewhere to delete the request from, and a suite naming none
does not.
