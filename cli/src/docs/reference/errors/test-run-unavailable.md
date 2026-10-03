---
title: A test runs only where this toolchain can build it
message: this toolchain cannot build a {platform} test binary
reproduction: none
---
# A test runs only where this toolchain can build it

```text
error: this toolchain cannot build a macos test binary [test-run-unavailable]
```

## What to do

Give this invocation a backend it can use, or say out loud that the suite runs
on JavaScript.

- `buri test --output=js` says it for one invocation and changes nothing in the
  repository.
- `test { backends: [JS] }` says it for the suite, which is right when the suite
  belongs there for good.
- A suite that asked for `NATIVE` in `test.backends` can drop it from the list.

Or fix the toolchain. The two profiles need different things:

- **debug** wants the `backend-stencil` feature (on by default), a stencil
  library for this host's triple, the runtime archive `cargo build -p buri`
  compiles, and a C compiler on `PATH` — `cc`, or whatever `CC` names.
- **release** wants `backend-llvm`, which is off by default and needs LLVM 21
  installed with `LLVM_SYS_211_PREFIX` set. A toolchain without it refuses
  rather than quietly handing the release build to the development backend.

## Why

`buri test` runs a suite that names no backend on the host, natively. Nothing
about your program is wrong: what's missing is a piece of this toolchain.
