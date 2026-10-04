# The crate split

The toolchain was one crate, `buri`. Any edit recompiled all 170k lines, and
every test binary waited on that. Now it's a stack of crates: Cargo compiles
independent ones in parallel, an edit recompiles only its crate and the ones
above it, and the compiler enforces the layers.

```text
buri-hash         crates/hash         table hasher, SHA-256, ActionKey
buri-stdlib       crates/stdlib       the embedded standard library and bundled platforms
buri-docs         crates/docs         every page under src/docs/, their catalogs, the markdown renderer
buri-diagnostics  crates/diagnostics  diagnostics, the source map, json, parallel
buri-syntax       crates/syntax       lexer, parser, tree, formatter, layout
buri-project      crates/project      build files, textproto, platforms, languages, the graph's vocabulary
buri-semantics    crates/semantics    the checker, module data, stdlib questions that need types
buri-middle       crates/middle       monomorphize, rc, lower, the IR, intrinsic keys
buri-backend      crates/backend      the Backend trait, targets, runtime table and symbol rule
buri-js           crates/js           the JavaScript backend
buri-stencil      crates/stencil      the copy-and-patch backend, and build.rs for its stencils
buri-llvm         crates/llvm         the LLVM backend and `inkwell`, behind `backend-llvm`
buri              cli/                build system, commands, LSP, `buri docs`, the binary
```

Each crate depends only on crates above it. `buri-js`, `buri-stencil` and
`buri-llvm` sit side by side and compile in parallel.

## Paths didn't change

Every moved file keeps its module path. `buri` re-exports each crate at the old
spot, so `buri::parsing::parser::parse` and `crate::compiler::middle::rc` still
work for tests, benches and the website. Inside a crate, the root imports the
lower crates' modules under their old names:

```rust
// crates/middle/src/lib.rs
use buri_diagnostics::{diagnostics, ice, parallel};
use buri_hash::hash;
```

So the files themselves barely changed. Only references that crossed a layer
the wrong way had to move.

## What moved to break a cycle

- The docs tree, `cli/src/docs/` to `crates/docs/src/docs/`. `Diagnostic::templated`
  reads its wording from `reference/errors/<code>.md`, so the pages sit below
  the diagnostics. A published crate can only embed files under its own
  directory, which is why the files moved rather than just the code.
  `documentation::embedded` holds the pages the commands embed.
- The standard library table and sources, to `crates/stdlib/`. The source map
  names standard library files and the formatter reads `core/character`'s
  printable table. `defining_module` and the platform-entry questions need the
  checker's types, so they stay in `buri-semantics`.
- `Severity`, into the frontmatter that declares it. `diagnostics` re-exports it.
- `ActionKey`, into `buri-hash`. A backend names its output by one.
- The host-platform questions (`host_arch`, `host_platform`,
  `host_native_platform`), into `build::buildfile`. `link` and `driver`
  re-export them.
- The graph's vocabulary (`PackageId`, `TargetId`, `ModuleLocation`, …) into
  `buri-project`. `Workspace` stays in `buri` because loading one runs
  generators and tools. The checker reads it through `build::workspace::Packages`:
  `package`, `resolve_module`, `declared_entries`.
- `Role`, `ModuleData`, `Loaded` and `Unit`, into `buri-semantics`. The module
  `Loader` stays in `buri`.
- The archive's capability gaps. The runtime archive is `buri`'s, so the native
  backends answer only from their own surface and `backend::WithRuntime` adds
  `networking_gap` and `cryptography_gap`. `backend::stencil::Stencil` and
  `backend::llvm::Llvm` name the wrapped types, so callers didn't change.

## What stayed in `buri`

- `cli/build.rs` and the runtime archive. `runtime_native`'s archive half,
  `build::musl` and `build::runtime_src` read `OUT_DIR`, so they stay beside the
  script. `cli/runtime/` and its `manifest.toml` didn't move.
- `backend::select`, because it names all three backends.
- Unit tests that compile a snippet through `driver::analyze_snippet`. They
  moved from `resolve`, `expressions`, `exhaustiveness`, `rc`, `lower`,
  `derives` and `park` into `cli/src/compiler/tests/`. A few internals they
  inspect became `pub`.

The stencil library is built by `crates/stencil/build.rs` now, so it and the
runtime archive build alongside the front end rather than in front of it.

## Publishing

`cargo package -p buri --list` still lists `runtime/**`, and every crate packages
on its own. A registry release publishes the crates bottom-up in the order
above, which is why each internal dependency has a `version` next to its `path`.
