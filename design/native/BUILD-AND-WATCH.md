# Building the toolchain, and `buri test --watch`

Three things this document settles: what the dependency policy becomes now that
it cannot be "none", how the toolchain is built and shipped with two optional
backends, and how a watch mode works given that the incremental test cache
already exists.

## 1. The dependency policy ends, and what replaces it

The current policy is stated in the root `Cargo.toml`:

> `buri` itself has no dependencies at all — the toolchain is pinned by hash, and
> a dependency tree is a second thing to pin

and again in `flake.nix`:

> The toolchain has no dependencies on purpose (see the root `Cargo.toml`), so
> the lockfile names one package -- this crate -- and vendoring fetches nothing.
> There is no `cargoHash` to keep in sync because there is nothing to hash.

That is now false, and it has to be replaced by something rather than quietly
weakened. The replacement is a **bar**, not a list, because a list is a thing
people add to.

> **A dependency is admissible only if it is a code generator or a platform
> interface that this repository could not reasonably write, it is behind a cargo
> feature the default build can turn off, and its absence degrades the toolchain
> rather than breaking it.**
>
> Everything that is an *algorithm* stays in-tree. SHA-256, the hasher, JSON, the
> lexer, the parser, the minifier, the SAT-free exhaustiveness checker and the
> file-change detector are all algorithms, and they are all already written here.
> A crate that would save a hundred lines is not admissible; a crate that
> encapsulates a million is.

The three clauses each rule something out. "Code generator or platform interface"
rules out convenience (`anyhow`, `clap`, `serde`). "Behind a feature" rules out
anything that would make `cargo install buri` need a system library. "Degrades
rather than breaks" is what makes the second clause enforceable: if turning a
feature off broke the toolchain, the feature would not stay off.

### 1.1 The admitted set

| Crate | Feature | Why it clears the bar |
|---|---|---|
| `cranelift-codegen`, `-frontend`, `-module`, `-object`, `-native` | `backend-cranelift` | A retargetable code generator with four backends. Not writable here at any scale. |
| `inkwell` (and `llvm-sys` transitively) | `backend-llvm` | Bindings to LLVM. Same, more so. |
| `target-lexicon` | both | Comes in with Cranelift; a triple parser is small, but forking one to disagree with Cranelift's is worse than depending on Cranelift's. |
| `object` | — | **Not a direct dependency.** `cranelift-object` depends on it and re-exports it, so version skew is a compile error. Nothing in this design needs it on the LLVM path — `TargetMachine::write_to_memory_buffer` produces the object. |

That is the whole list. `Cargo.lock` names four packages plus their closures, the
flake gains a `cargoHash`, and both of the comments quoted above are rewritten to
state the bar instead of the absence.

### 1.2 The file watcher: not a dependency, because there is no watcher

`notify` is the obvious dependency for `--watch`, and it does not clear the bar —
but the interesting part is that this design does not need it, and the reason is
specific rather than ideological.

A general-purpose file watcher is genuinely hard, and the evidence is one-sided:
**Zed maintained its own FSEvents crate for years and has now migrated to
`notify`**; **Zig's `zig build --watch` used kqueue on macOS and did not fire at
all when saving from VS Code or Zed** (`ziglang/zig#21905`) until it was rewritten
on FSEvents in 2025. Editor atomic saves produce `Create` + rename + `Remove` and
never `Modify(Data)`; inotify is inode-based so watching a file rather than its
parent silently dies on the first save; macOS enforces an undocumented FSEvents
path limit at roughly `RLIMIT_NOFILE / 10`. Hand-rolling that is not admissible
under any policy, and this document is not proposing it.

**The proposal is not to watch.** The build already knows the exact set of files
that can affect an action, because it reads and hashes every one of them to
compute a key: `contribute` enumerates a rule's sources and proto sources, and
`test_key` adds the suite's sources, its data, and the closure of every library
the suite's own `dependencies` name. (That last was **missing from both** until
wave 3c — a test-only helper was compiled into the suite and hashed into
nothing, so editing one served the previous verdict. It is one bug in two
places, and the two are fixed in one place: `watch::inputs` and `test_key` walk
the same edges, which is the property this section is claiming.) So the watch set
is a *declared, enumerated, usually-few-hundred-file* set,
and detecting change in it is a `stat` sweep:

```
every 150 ms:
    for each declared path: lstat -> (mtime_ns, size, inode)
    if the tuple set differs from the last sweep: something moved
```

At a few hundred files that is well under a millisecond of syscalls. At ten
thousand it is a few milliseconds, still under 3% of one core at this interval.
It cannot miss an edit that is on disk when the sweep runs, it has no platform
code, and it is immune to every item in the paragraph above — an atomic save
changes the inode, which the tuple notices; a rename changes the set; a file
appearing that no rule declares is invisible, which is correct, because a file no
rule declares cannot change an action.

Two properties fall out for free, and both are things a real watcher has to work
for:

- **The build cannot re-trigger itself.** Only declared *sources* are polled, and
  the build writes to `.buri/`. Trunk needs a one-second post-build cooldown and
  bacon a grace period specifically to stop their own writes retriggering; here
  there is nothing to cool down. (`--accept` is the exception — it writes golden
  files into the source tree — and §4.3 refuses to combine it with `--watch`.)
- **`.git/`, `target/` and `node_modules/` are not watched**, without an ignore
  file, because they are not declared. That is the single most common source of
  watcher pathology, absent by construction.

The threshold at which this stops being right is a repository with tens of
thousands of declared sources, and the growth path is `notify` behind a
`watch-events` feature with the poller as the fallback — which is the arrangement
every serious Rust build tool ends up at anyway (watchexec, dioxus and
rust-analyzer all pair `notify` with a poll watcher for the cases it fails on).

## 2. Cargo features, and keeping `cargo install` easy

```toml
[features]
default          = ["backend-cranelift"]
backend-cranelift = ["dep:cranelift-codegen", "dep:cranelift-frontend",
                     "dep:cranelift-module", "dep:cranelift-object",
                     "dep:cranelift-native"]
backend-llvm      = ["dep:inkwell"]
```

There is no `backend-js` feature. The JavaScript backend is always compiled in:
it needs nothing, it is what `host_platform()` still returns (`driver.rs:228-232`),
and a feature whose only possible value is "on" is a flag nobody should have to
read.

**`backend-cranelift` is on by default.** Cranelift is pure Rust with no system
dependency and cross-compiles anywhere Rust does, so having it on costs a
contributor nothing but compile time, and having it off would mean the default
toolchain cannot build a native artifact at all.

**`backend-llvm` is off by default.** It needs LLVM 21 installed and
`LLVM_SYS_211_PREFIX` set (CODEGEN-LLVM.md §8). `cargo install buri` must not
require that, so it does not.

### 2.1 What a toolchain without LLVM does

It refuses `--release` for a native platform, with a diagnostic naming the
feature — it does not fall back to Cranelift. A `--release` build that silently
produced different code depending on how the compiler happened to be installed is
the same class of bug as an unpinned toolchain, and this repository already
refuses to run against a compiler it was not pinned to.

The hazard that would normally follow — two `buri` binaries with identical
sources and different capabilities — is already closed by a mechanism that
exists. `build/toolchain.rs` hashes **the running executable** and refuses to run
if `REPO.buri`'s `sha256` does not match (`TODO.md:1175-1189`), and a binary built
with `backend-llvm` is a different executable with a different hash. So a
repository pinned to an LLVM-enabled toolchain cannot be built by one without,
and nobody has to have thought about it.

`Backend::identity()` (ARCHITECTURE.md §3) closes the other half: the LLVM
version enters every release `codegen` key, so two LLVM-enabled toolchains built
against different LLVMs do not share cache entries.

### 2.2 The runtime archive

`cli/runtime` is a Rust static library with a C ABI (VALUE-MODEL.md §10), built
for the host by `cli/build.rs`:

```
rustc --crate-type=staticlib --crate-name=buri_rt --edition 2024 \
      -C opt-level=3 -C panic=abort -C debuginfo=0 -C lto=fat \
      -C codegen-units=1 -C metadata=buri_rt -C extra-filename= \
      --remap-path-prefix=<manifest dir>=. \
      --target <host triple> -o $OUT_DIR/libburi_rt.a cli/runtime/lib.rs
```

and `include_bytes!`-ed into the binary through
`backend::runtime_native::ARCHIVE`. `rustc` is already required to build the
toolchain, so this adds no tool; it is not a cargo dependency, so it adds nothing
to the lockfile; and `--target <host triple>` names the one triple this build
supports, which is why cross-compilation is refused rather than half-working
(ARCHITECTURE.md §9).

The three flags past the obvious ones each fix something measured rather than
guessed, on `aarch64-apple-darwin`:

- **`-C lto=fat`.** A staticlib bundles the whole of `std`, and the archive is
  embedded in every `buri` binary. Without it the archive is 17.7 MB; with it,
  6.0 MB, for 2.6 seconds of build time once per toolchain build. Nothing is
  lost: every entry point is `#[unsafe(no_mangle)]`, so LTO has no root to
  internalize away, and the linked artifact is dead-stripped either way — a C
  driver linking the whole surface comes out at 470 KB.
- **`-C metadata=buri_rt -C extra-filename=`.** Without these, the archive's
  bytes depend on the *output path*, because the member names inside it carry
  rustc's symbol hash. Two `OUT_DIR`s produced archives differing by a few dozen
  bytes, and the difference would have been invisible until the hash below made
  it a cache miss. With them, and with `--remap-path-prefix`, two builds of the
  same tree produce byte-identical archives — which is what
  `--check-reproducible` (ARCHITECTURE.md §7) needs from every input to a link.
- **`-C panic=abort`** because SPEC 6.10 says an abort is a write to standard
  error and an exit, never an unwind, so the tables would be dead weight in
  every artifact.

The archive's SHA-256 enters the `link` key (ARCHITECTURE.md §6.2), so editing the
runtime relinks every artifact and recompiles none — which is right, because the
runtime is linked and not compiled against.
`backend::runtime_native::archive_hash()` is what supplies it.

**On a host with no runtime** — anything that is not macOS or Linux — the build
script writes an *empty* archive and `runtime_native::AVAILABLE` is false. That
is the "degrades rather than breaks" clause of §1's dependency bar applied to
the runtime itself: `cargo build -p buri` succeeds everywhere, the JavaScript
backend is unaffected, and the native backends are the only thing that is
missing, which they would have been anyway.

## 3. nix and CI

### 3.1 devShell

```nix
llvm = pkgs.llvmPackages_21.llvm;
devShells.default = pkgs.mkShell {          # mkShell, not mkShellNoCC: llvm-sys needs a cc
  packages = [ pkgs.cargo pkgs.bun pkgs.elan
               pkgs.cmake pkgs.ninja pkgs.abseil-cpp pkgs.zlib pkgs.pkg-config
               llvm.dev llvm pkgs.libxml2 pkgs.libffi
               pkgs.lld ]
             ++ pkgs.lib.optional pkgs.stdenv.isLinux pkgs.mold;
  LLVM_SYS_211_PREFIX = "${llvm.dev}";
};
```

Four notes, each of which is a mistake someone would otherwise make:

- **`llvmPackages_21`**, pinned deliberately (CODEGEN-LLVM.md §8). The flake's
  `nixos-25.05` provides 18.1.8, 19.1.7 (the default), 20.1.8 and 21.1.2, and no
  22 — so pinning LLVM 22 would require bumping the flake's nixpkgs, which is a
  change to how the whole toolchain is built in service of a codegen decision.
- **`llvm.dev`**, not `llvm`. The `.dev` output carries `bin/llvm-config` and the
  headers; the default output does not, and the failure does not say so.
- **`mkShell` rather than `mkShellNoCC`.** `llvm-sys`'s build script needs a C++
  compiler, and the link step shells out to `cc` (CODEGEN-CRANELIFT.md §7.3).
  This is a change to the existing shell, which is `mkShellNoCC` today.
- **`mold` on Linux only** (2.39.1 on 25.05). It is ELF-only and does not support
  macOS. `lld` follows the default `llvmPackages`, so it is 19.1.7 here — which is
  fine, because a linker's version need not match the compiler's.

### 3.2 `packages.default`

Built **with** `backend-llvm`, because a `nix build` produces the release
toolchain and a release toolchain must be able to produce release artifacts. That
means `nativeBuildInputs = [ llvm.dev ]`, `LLVM_SYS_211_PREFIX`, and — the change
the current flake comment is about — a real `cargoHash`, since vendoring now
fetches four crates and their closures.

### 3.3 CI

The existing `test` job (`.github/workflows/ci.yml`) runs on `ubuntu-latest` and
`macos-latest` and installs nothing but Rust, bun and node. It gains:

- `mold` on the Linux runner (`apt-get install -y mold`) and nothing on the macOS
  runner, which has Apple's `ld` and no need for another.
- LLVM 21 on both, via `KyleMayes/install-llvm-action` or `brew install llvm@21`
  / `apt.llvm.org`, with `LLVM_SYS_211_PREFIX` exported.
- `cargo test -p buri --no-fail-fast --features backend-llvm`, so both backends
  are exercised. `cranelift_and_llvm_agree` (ARCHITECTURE.md §4) is the test that
  makes the second feature worth turning on.

A third job, `no-llvm`, runs `cargo build -p buri` with default features on
ubuntu with no LLVM installed. It is the test that `cargo install buri` still
works, and it is a job rather than an assertion because "it builds without the
optional system library" is only true if something builds it that way.

### 3.4 Without nix

**As landed (wave 2a).** The default build needs *nothing* new. Cranelift is
pure Rust, so `cargo build -p buri`, `cargo test -p buri` and `cargo install
buri` work on a machine with a Rust toolchain and a C compiler and no LLVM, no
lld and no mold anywhere on it. That is the whole point of §2's default set, and
it is the state a contributor is in unless they go looking for the other one.

`cc` is the one thing that is not optional, and it is not new either: the link
step drives the platform C compiler (CODEGEN-CRANELIFT.md §7.3) and
`cli/tests/runtime_native.rs` already compiled a C driver against the runtime
archive before this wave. It is Xcode's command-line tools on macOS
(`xcode-select --install`) and `build-essential` on Debian-likes.

Everything below is for the two things the default build does not do: build the
**LLVM** backend, and link with something faster than the system linker.

| | macOS | Debian / Ubuntu | Fedora / Arch |
|---|---|---|---|
| LLVM 21 | `brew install llvm@21` | `apt.llvm.org`'s `llvm-21-dev` | `dnf install llvm-devel` / `pacman -S llvm`, if it is 21 |
| `LLVM_SYS_211_PREFIX` | `$(brew --prefix llvm@21)` | `/usr/lib/llvm-21` | `llvm-config --prefix` |
| lld | comes with `llvm@21` | `apt install lld` | packaged with LLVM |
| mold | not applicable | `apt install mold` | `dnf install mold` / `pacman -S mold` |

Three things about that table are worth stating rather than leaving to be
discovered:

- **The version should be 21, and as landed nothing forces it.** CODEGEN-LLVM.md
  §8 asks for `llvm-sys`'s `strict-versioning`, so that an LLVM 22 on the
  machine is a build failure rather than a silent substitution — which is the
  behaviour a compiler whose central claim is byte-identical output should have.
  It is *not* enabled by the manifest wave 2a landed, because inkwell exposes no
  passthrough for it (its `llvm21-1` feature expands to `llvm-sys-211` and
  nothing else), and reaching it would mean a second direct dependency on
  `llvm-sys` — which is a second thing behind §1's bar, for a flag. Until that
  is decided, `Backend::identity()` is what stops two toolchains built against
  different LLVMs from sharing cached objects, and `llvm-config --version` is
  what a contributor should check.
- **`LLVM_SYS_211_PREFIX` points at the directory containing `bin/llvm-config`**,
  not at a lib directory. If `llvm-config --version` prints `21.1.x`, its prefix
  is the right answer: `export LLVM_SYS_211_PREFIX=$(llvm-config-21 --prefix)`.
- **mold is Linux-only.** It is ELF-only and fails with "mold does not support
  macOS"; the Mach-O fork, `sold`, was archived in November 2024 with its own
  author recommending Apple's linker. macOS contributors want `lld` or nothing,
  and nothing is a perfectly good answer — §7.3's fallback is that with neither
  mold nor lld present everything works, more slowly, and no flag has to be set
  to get a working build.

## 4. `buri test --watch`

### 4.1 Watch mode is a loop over the cache

The incremental test cache already exists and already does the hard part.
`run_on` (`test.rs:263-305`) computes `test_key`, consults `Cache`, and returns
cached results with `Provenance::Cache` when the key hits; the summary already
prints "`n` cached" (`test.rs:161-165`). Only a clean run is cached, deliberately
— "a failure is what you are trying to fix, and re-running it should re-run it"
(`test.rs:397-400`) — which is exactly the behaviour a watch loop wants.

So **watch mode is a loop around `cmd_test`, and the incrementality is the
cache's rather than the loop's**. There is no "affected target" computation to
write: a target whose inputs did not move gets a cache hit and costs a hash of
its sources, and a target whose inputs moved gets re-run. The machinery for
deciding what to re-run is the same machinery that decides what to re-run
without `--watch`, which is the only way to be sure the two agree.

The loop:

```
open a session
compute the watch set (§4.2)
run the suites once, print
loop:
    sweep the watch set every 150 ms
    when the sweep differs from the last one:
        keep sweeping until two consecutive sweeps agree   (the settle window)
        if a BUILD.buri or REPO.buri moved: reopen the session, recompute the set
        run the suites, print
```

### 4.2 What is watched

Per selected target, the union of:

- every path `contribute` enumerates for every member of the target's closure —
  the rule's entry point, its `sources`, its `proto_sources`, and its
  `testing/` sources (`actions.rs:190-213`);
- every path `test_key` enumerates — the suite's `sources`, its `data`, and the
  closure of every library its `test { dependencies }` and
  `testing { dependencies }` name;
- every package's `BUILD.buri`, for every package in the closure;
- the repository's `REPO.buri`.

The first two are exactly the inputs the keys are computed from, so a change that
does not move a key does not exist as far as the loop is concerned — and a change
that does move one is guaranteed to be seen, because the same enumeration
produced both.

The last two are not in any key's *input* list but change the graph itself: a new
dependency edge, a new source, a changed tag vocabulary. A change to either
re-opens the `Session`, because `Workspace::load` is what reads them
(`session.rs:46-60`) and a `Session` holds a loaded graph rather than a
directory.

The parse cache (`Session::parsed`) is discarded on reopen and kept otherwise,
which is what makes a second run of an unchanged suite cost nothing.

### 4.3 Timing, and the flags it refuses

**150 ms sweep, and a settle window of one further quiet sweep.** So a save is
acted on between 150 and 300 ms after it lands, and a sequence of writes — a
formatter rewriting twelve files, a `git checkout` — is coalesced into one run.
For comparison, watchexec debounces at 50 ms, trunk at 25 ms plus a one-second
post-build cooldown, and bacon at 15 ms; those are event-driven and pay per event,
where this pays per sweep, so the number is a sweep interval rather than a
debounce and is correspondingly larger. It is not configurable in v1. A flag
here is a flag nobody can choose a value for.

Three combinations are refused, at argument parsing, with exit 2:

- **`--watch --force`.** `--force` turns every cache hit into a run
  (`test.rs:278`), so every keystroke would re-run every suite in the selection.
  That is the opposite of the mode.
- **`--watch --accept`.** `--accept` is "the one mode that writes to the source
  tree" (`test.rs:398-400`). A mode that rewrites golden files on a timer is a
  mode that silently accepts a regression while you are reading the failure.
- **`--watch` without a TTY.** A watch loop in CI is a hung job. The check is on
  stdout being a terminal, and the diagnostic says so and names `buri test`.

### 4.4 The terminal

- **The screen is not cleared.** Scrollback is where the failure you are fixing
  is; the previous run's output is the thing you are comparing against. Each run
  is separated by a rule with the time on it, which is one line and is greppable:
  `── 14:02:31 ─────`.
- **One summary line per run**, in the format `cmd_test` already prints
  (`test.rs:165`): `12 passed, 1 failed, 0 skipped (0.4s, 8 cached)`. Failures
  print above it exactly as they do without `--watch`, through `report_failure`,
  so a suite's output does not depend on which mode it was run in.
- **A run with nothing to do prints nothing at all.** If every suite was served
  from the cache and every one passed, the loop is silent. A watch mode that
  prints on every sweep trains you not to read it.
- **`--explain` works**, and in watch mode it is the most useful it has ever been:
  one `test` line per suite per run with `cached` or `run`, which is the
  incrementality claim being observable rather than asserted
  (`arguments.rs:76-79`).
- **Ctrl-C exits 0**, whatever the last run said. The exit status of a watch loop
  is about the loop; a red suite is on the screen, and encoding it in `$?` would
  make `buri test --watch` unusable in a shell with a prompt that shows the last
  status.

### 4.5 What it is not

Not a hot-reload, not a REPL, and not a language server. It re-runs `buri test`
and it does not keep a compiled program alive between runs. The reason to say so
is that the machinery that would make it more — a persistent process holding the
checked standard library between runs — is the same machinery
`TODO.md:1423-1427` names as still missing ("nothing shares work between
processes"), and it is a separate, larger piece of work whose first customer
would be the language server rather than this.

## 5. Implementation waves

Sized so each item is one agent's work, ordered so no two items in a wave write
the same file. The collision map is the point of the table: it is what makes
parallel implementation safe rather than optimistic.

### Wave 0 — the middle end. One agent, alone. **Landed.**

Nothing starts until this lands, and it gets harder every time `generate.rs`
grows — `TODO.md:1739-1743`, with a date on it.

- `compiler/transform/` → `compiler/middle/`, `optimize.rs` → `inline.rs`, and
  `middle/mod.rs` declares **every** module later waves add, each a stub. It
  also holds `middle::run` and `middle::native`, which already *call* the stubs
  — so a wave 1 item fills a function body and touches nothing else. This is
  what makes the later waves collision-free, and it is the reason it is worth
  doing up front.
- `backend/generate.rs` + `javascript.rs` + `intrinsics.rs` + `runtime.js` move
  under `backend/js/`, which gains a `mod.rs` holding the `Backend` impl.
  `backend/{cranelift,llvm}/mod.rs` are created as feature-gated stubs, so both
  gates are compiled before there is anything behind them to break.
- `backend/mod.rs` gains `Backend`, `Linker`, `Emitted`, `Options`, `Target`,
  `select`, and `Profile` moves into it from `generate.rs:36`.
- `cache.rs` gains `Action::Codegen`, and `KeyBuilder` gains `backend` and
  `linker`. `Backend::identity()` enters every `link` and `test` key, so a
  backend change invalidates the cache.
- `Cargo.toml` gains the two features, empty, and the dependency policy becomes
  the bar in §1 rather than "none at all".
- The JS backend implements `Backend`, and `actions.rs::emit` (`actions.rs:334`)
  goes through the trait. `actions.rs` also gains `codegen_key`, so wave 2c
  writes a body rather than a shape.

**Not** in wave 0: `arm_chain`, closure conversion and DCE stay in the
JavaScript backend until wave 1d moves them. Wave 0's whole value is that it
changes no emitted bytes — `golden_javascript` passes unblessed across it — and
mixing a behaviour change in would cost that.

**Touches:** everything. **Blocks:** everything.

### Wave 1 — five in parallel.

| # | Item | Files it writes |
|---|---|---|
| 1a | `middle::ir` and `middle::lower` — the block-argument SSA CFG (CODEGEN-CRANELIFT.md §1) | `middle/ir.rs`, `middle/lower.rs` |
| 1b | `middle::layout` — VALUE-MODEL.md, as a memoised table, plus the `Alloc` cost model (MEMORY.md §7.1) | `middle/layout.rs` |
| 1c | `cli/runtime` — the C-ABI runtime and the `build.rs` that builds it | `cli/runtime/**`, `cli/build.rs`, `cli/Cargo.toml` |
| 1d | `middle::{decision, closures, dce, tail_calls}` — the tree passes every backend or the native branch needs, and the tail-call *rewrite* that replaces the emitter's `Plan` consultation | `middle/decision.rs`, `middle/closures.rs`, `middle/dce.rs`, `middle/tail_calls.rs`, `backend/js/generate.rs` |
| 1e | `middle::{derives, rc}` — generated derives (VALUE-MODEL.md §9) and own/borrow inference with reuse (MEMORY.md §5.2-5.3) | `middle/derives.rs`, `middle/rc.rs` |

**Collisions:** none. 1a, 1d and 1e all add files under `middle/`, but
`middle/mod.rs` was written in wave 0 and none of them edits it — wave 0 left
the pipeline already calling every stub, so filling one in is a change to one
file. 1c is the only writer of `cli/Cargo.toml` in this wave, and 1d is the only
writer of `backend/js/generate.rs`, which it edits to *delete*: `arm_chain`, the
`Plan` consultation, and the dead-code pass by name all leave together.

**Note:** 1a depends on 1b's *interface* but not its content — the lowering asks
`layout(ty)` for sizes and offsets. Agree the signature in wave 0's stub and both
proceed.

### Wave 2 — three in parallel.

| # | Item | Files it writes |
|---|---|---|
| 2a | The Cranelift backend | `backend/cranelift/**`, root `Cargo.toml` (`backend-cranelift` deps) |
| 2b | The LLVM backend | `backend/llvm/**` |
| 2c | The link step, the object cache, and the manifest | `build/link.rs`, `build/actions.rs`, `build/cache.rs` |

**Collisions:** 2a and 2b would both want `Cargo.toml`. Resolved by having **2a
own it** and land both dependency blocks — the `inkwell` line is three lines and
2b does not need to write them. `cache.rs`'s `Action::Codegen` was added in wave
0, so 2c edits `cache.rs` only for the key builders.

**Note:** 2a and 2b are the two that must agree, and what they agree on is
`middle::layout` and `middle::ir`, both frozen in wave 1.
`cranelift_and_llvm_agree` is written by whichever lands second.

### Wave 3 — three in parallel.

| # | Item | Files it writes |
|---|---|---|
| 3a | Native `--check-reproducible`: per-object comparison, `-no_uuid`, relative debug paths (ARCHITECTURE.md §7) | `commands/build.rs` |
| 3b | `buri test --watch`: the poller and the loop | `build/watch.rs`, `commands/test.rs`, `commands/mod.rs`, `commands/arguments.rs` |
| 3c | The `host_platform()` switch, the SPEC amendment, the docs, and the golden re-record | `compiler/driver.rs`, `SPEC.md`, `cli/src/docs/**`, `cli/tests/**` goldens |

**Wave 3c, as landed.** Four things, and the third and fourth were not in the
plan:

1. The `host_platform()` switch, gated on `native_ready` (ARCHITECTURE.md §4 has
   the amended version, including why the default output stays `JS`), with
   `buri run` executing a native artifact directly and `buri test` running a
   native suite as a binary.
2. The SPEC amendment (VALUE-MODEL.md §11, now marked applied), and `num.buri`'s
   2^53 comment with it.
3. **The pipeline seam.** Wave 2b reported that `Backend::emit` takes `&Program`
   and `middle::native` needs `&mut`, so the composition had to move into the
   build system: `actions::prepare` is it, and it is what both `actions::emit`
   and `actions::objects_of` call. Without it, a native target reached through
   `actions::emit` — which is the path `buri test` takes — would have been
   handed a program that never went through closure conversion.
4. **The `test_key` hole** (§4.2), found by wave 3b and fixed here because the
   key and the watch set are one enumeration.

**Not** landed: the golden re-record for a *Linux* host. Two fixtures name
`linux` as the platform this toolchain cannot produce, and on a Linux machine it
now can. See ARCHITECTURE.md §4's last paragraph.

**Collisions:** 3b is the only writer of `commands/mod.rs` and
`commands/arguments.rs`. 3a and 3c both remove a "the backend is not implemented"
refusal — `build.rs:169-173` is 3a's, `test.rs:232-247` is 3b's, and
`actions.rs:45-54` was 2c's. Assigning one refusal to each is deliberate; the
alternative is three agents editing one condition.

**Note:** 3c is the largest by volume and the smallest by thought. It is last
because every golden file it re-records depends on 2a and 2b being settled.

### Wave 4 — the allocator types.

`GeneralPurpose`, `Arena` and `FixedBuffer` in `core/cap`, the `Alloc` accounting
in the runtime, and the cost model written into `core/cap`'s own source
(MEMORY.md §7). One agent. It is last because it is the only item that needs both
a working native runtime and a settled cost model, and it is the item
`TODO.md:1448-1481` has been holding.

### What is not in any wave

DWARF (CODEGEN-LLVM.md §7 sketches it, CODEGEN-CRANELIFT.md §5 declines it),
cross-compilation (ARCHITECTURE.md §9 refuses it), ThinLTO (ARCHITECTURE.md §5.2
leaves the door open), general niche discovery (VALUE-MODEL.md §6), and
small-string optimization (VALUE-MODEL.md §3.2). Each is named where it is
declined so that a later reader finds the reason next to the decision rather than
in a plan.
