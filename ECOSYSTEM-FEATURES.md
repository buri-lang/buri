This details the ecosystem features:

- JavaScript output — **done.** Optimized per the ReScript survey (TODO.md, "The
  JavaScript backend"): shared middle-end passes, TCO including `&&`/`||`/`??`,
  measured runtime wins, two miscompiles found and fixed.
- Executable (macOS and Linux) output — **in progress.** Designed in
  `design/native/` (LLVM via inkwell for release, Cranelift for dev/tests,
  incremental linking via lld/mold, shared middle-end); implementation waves
  running.
- Test runner built in — **done.** Injected hermetic capabilities
  (`core/testing/context`), `--accept`, `--filter`, per-platform runs,
  `timeout_seconds`, caching with `--explain`. Watch mode designed
  (`design/native/BUILD-AND-WATCH.md`), lands with the native waves.
    - Any we can easily assert changes to anything in the context without doing
      actual I/O operations — **done** (`captureOut`, `files`, `clockAt`,
      `stdinBytes`, …).
- LSP — **done.** Diagnostics (front-end and build-graph), hover, definition,
  documentSymbol, formatting, completion, code actions; recorded sessions in
  `cli/tests/repositories/lsp/`. Untested against a real editor.
- Linter built-in — **done.** Full catalogue in `docs/build/cli.md`;
  `lint --fix`.
- Code formatter built-in — **done.** Wadler/Prettier document algebra,
  4-space indent, paired input/expected corpus (81 cases), opinionated with no
  configuration. The repository's own corpus is not yet reformatted with it.
- Mono-repo support (declaring libraries, deps, and binary build outputs,
  probably configured in textproto) — **done.** `BUILD.buri`/`REPO.buri`,
  visibility, tags/platforms, `buri gen` managing six fields, queries.
- Protobuf serialization / deserialization by importing a .proto file directly
  (does not need to be integrated into protoc, we can just do this ourselves),
  including json and binary serialization/deserialization — **done.**
  Editions 2026 required (proto3/proto2 refused), generated Buri codecs, both
  formats both directions, official conformance suite green
  (`cli/tests/proto/`: 970 successes, 0 unexpected failures).
- Zed language extension — **done**, except `editors/zed/extension.toml` still
  pins a placeholder commit: the tree-sitter grammar (now generated from
  `grammar.ebnf`, the single source of truth) must be published as its own
  repository before the extension installs as anything but a dev extension.
- Generate documentation from doc comments — **done** (`buri docs`, assembled
  README/SPEC, reference pages).
- Write tests inside documentation comments — **done** (doctests compile and
  run from `///`/`//!` and from the prose docs).
- Robust standard library — **done** except allocators (see below).
    - Networking and HTTP — `core/http`, `core/host` (JS backend).
    - JSON serialization/deserialization — `core/json` plus
      `derive ToJson`/`FromJson`.
    - UTF-8 text processing — `core/str`, `core/char`, `core/bytes`.
    - Cryptography — `core/crypto`.
    - Time and Date utilities — `core/time`, `core/date`.
    - Collections: queues, hash maps, bit sets, simd, Struct of Arrays
      (MultiArrayList or something, basically the same as an array of structs
      but laid out differently in memory for performance reasons) — queues,
      maps, sets, bitsets, simd done; SoA not built (needs a use case).
    - Multiple allocators (GeneralPurposeAllocator, arena allocator,
      FixedBufferAllocator) — **deferred to the native backend deliberately**
      (TODO.md records why: on JS they would measure nothing); the native
      memory design (`design/native/MEMORY.md`) makes them load-bearing, wave 4.

Not on the original list but part of the ecosystem now: nix flake +
Homebrew-tap packaging (`Formula/`), GitHub Actions CI (`.github/workflows/`),
a Lean 4 formalisation of the type system with an executable bridge
(`formal/`), and the official protobuf conformance harness
(`cli/tests/proto/`).
