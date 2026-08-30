# Building the toolchain, and `buri test --watch`

Three things this document settles: what the dependency policy becomes once it
cannot be a list, how the toolchain is built and shipped with one optional
backend beside the two that are always there, and how a watch mode works given
that the incremental test cache already exists.

## 1. The dependency policy ends, and what replaces it

The policy used to be "no dependencies at all", and native code generation ended
that: a retargetable code generator is not something this repository can write.
What replaced it is a **bar** rather than a list, because a list is a thing
people add to. The bar and the argument for each of its three clauses are in the
root `Cargo.toml`, where somebody about to add a dependency will meet them; they
are not restated here.

What belongs here is what the bar admitted, and why each one clears it — and,
since 2026-08-30, *which* of the two admitted sets admitted it. The bar is one
sentence applied twice, because "a crate a contributor installs" and "a crate
every user of this compiler ships inside their own binary" are not the same
decision. The root `Cargo.toml` states both halves; §1.1 is the first and §1.1.1
is the second.

### 1.1 The toolchain's admitted set

| Crate | Feature | Why it clears the bar |
|---|---|---|
| `inkwell` (and `llvm-sys` transitively) | `backend-llvm` | Bindings to LLVM. Not writable here at any scale. |

That is the whole list, and it is one row **off by default**, so the toolchain
`cargo install buri` builds has no dependency closure at all. It had one until
2026-08-29: `cranelift-{codegen,frontend,module,object,native}` behind
`backend-cranelift`, and `target-lexicon` and `object`, which came in with them.
Removing that backend (CODEGEN-STENCIL.md §13) took thirty-eight transitive
crates with it — over half of `Cargo.lock` — and restored "none at all" for the
default build without restoring it as a policy. The bar is still the bar, and
the flake still carries a `cargoHash` because a build **with** `backend-llvm`
still has something to hash.

The backend that took the debug seat admits nothing. `backend-stencil`
(CODEGEN-STENCIL.md) is on by default and its feature list is empty: the
copy-and-patch code generator is written in this repository, and the one thing
it needs from outside is a host `cc` — a platform interface in exactly the sense
the bar means, already required by the link step, and absent from the lockfile.
§2 has what having it on costs.

### 1.1.1 The runtime's admitted set, which is closed by an exact list

`libburi_rt.a` is linked into every native binary this compiler produces, so a
crate admitted to `cli/runtime/manifest.toml` is a crate admitted into
strangers' programs. Same bar, one extra clause: the set is **closed by an exact
list**, asserted as an equality by `dependencies_stay_behind_the_bar`, so that a
fifth crate — and equally a removal — is a failing test rather than a review
comment.

| Crate | Feature | Why it clears the bar |
|---|---|---|
| `tokio` | `net` | The reactor and the timer wheel. `epoll` and `kqueue` behind one readiness API, per platform; getting it subtly wrong presents as a hang. |
| `hyper` | `net` | HTTP/1.1 and HTTP/2 framing. `cli/runtime/http.rs` is a complete cleartext client, which is the easy half; HPACK, flow control and a correct server are not. |
| `rustls` | `net` | TLS 1.2 and 1.3 — the crate `cli/runtime/http.rs`'s header named as the growth path when it refused `https://`. |
| `tungstenite` | `net` | RFC 6455 framing and the handshake: a protocol with a specification and a conformance suite, not an algorithm. |

`net` is **on by default**, which is the opposite of `backend-llvm` and for a
reason the bar's third clause makes: turning `backend-llvm` off costs a release
code generator a contributor can do without, while turning `net` off costs a
*language capability*. So the degradation has to be a diagnostic naming the
missing effect rather than a link error, which is why the toolchain learns the
feature's state through `Backend::missing_intrinsics` instead of finding out at
`cc` time.

As of the slice that admitted them, **nothing referenced any of the four**.
`cli/runtime/net.rs` names one type from each and exports two entries that
answer "was this toolchain built with the networking stack"; no intrinsic key
mangles to a symbol in that file. That was deliberate, and it is what made the
cost measurable before anything depended on the answer: on
`aarch64-apple-darwin` the archive is 5 987 472 bytes with `net` off and
5 987 496 with it on. Twenty-four bytes, because `lto = "fat"` is whole-program
across the dependency rlibs and Rust code nothing reaches does not reach the
archive.

**`tokio` has since been linked, once and deliberately.** `cli/runtime/rt.rs`
is the carrier runtime — the reactor handle, the run baton, the carrier pool
with its 512 KiB stacks and the task table — and `Clock::sleepMillis` and
`Net::fetch` wait on it through `park_on`, so the reactor and its timer wheel
are in the archive on purpose:

| `aarch64-apple-darwin`, `libburi_rt.a` | bytes |
|---|---|
| before `rt.rs` | 6 035 480 |
| after | 6 220 904 |
| the reactor | +185 424 |

`mio` and `socket2` arrive with it and are tokio's platform layer rather than a
fifth and sixth dependency; the direct set is still four, and
`dependencies_stay_behind_the_bar` is what holds it there. The other three
crates are still reached by nothing, and the CI symbol check names them
still.

The one thing that would *not* be dropped is a dependency's **native** object
code, which `rustc` bundles into a `staticlib` whether anything references it or
not — measured at 842 544 bytes and twenty-four `.o` members for `rustls`'s
obvious crypto provider, `ring`. So `rustls` is admitted with no provider, and
picking one is the decision of the slice that makes `https://` work, where the
bytes are bought rather than spent. `.github/scripts/assert-runtime-archive.sh`
holds both halves in CI: a size budget, and a symbol table with none of the
crates nothing calls into in it.

**How the toolchain knows.** `cli/build.rs` writes `libburi_rt.a.features`
beside the archive and beside its digest — the Cargo features the archive was
built with, one per line, and empty when there is no archive at all.
`runtime_native::FEATURES` is an `include_str!` of it and `runtime_native::net()`
a whole-line lookup in that, which is the same "one `OUT_DIR`, one run of the script,
so the bytes and the facts about them travel together" argument the baked digest
is written for. Not a `--cfg`, for the reason the archive's *emptiness* rather
than a `--cfg` is the availability signal: conditional compilation would need a
`check-cfg` list to know about, and a fact that travels beside the bytes cannot
be paired with another build's.

What reads it is `Backend::missing_intrinsics`, on both native backends. The
`host.HostListen.*`, `host.HostSockets.*` and `host.HostTasks.*` family is
missing on a toolchain whose archive has no `net`, whatever the backend has a
body for, and `backend::split_networking` sorts that half out from the ordinary
"this backend has no implementation of" half at each of the two emission sites.
The refusal is `networking-not-available`, whose fix names the feature rather
than asking for a bug report: the program is fine and the toolchain is what has
to change. The refusal landed **before any of those keys existed**, on purpose,
so that the day one arrived it arrived with its diagnostic already written
rather than as an unresolved `buri_rt_*` symbol from `cc`. The first one is
`host.HostTasks.parallel`, and it is answered by `cli/runtime/rt.rs` — behind
`net`, beside the carrier pool it will fan out onto — so on a
`BURI_RUNTIME_NET=0` toolchain a program that calls `core/tasks` is refused by
name before code generation. That is what was designed, and it is now exercised
rather than only argued for. `host.HostListen.*` and `host.HostSockets.*` are
still ahead of their first key.

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
default         = ["backend-stencil"]
backend-stencil = []
backend-llvm    = ["dep:inkwell"]
```

There is no `backend-js` feature. The JavaScript backend is always compiled in:
it needs nothing, it is what `driver::host_platform` still returns, and a
feature whose only possible value is "on" is a flag nobody should have to read.

There is no `backend-cranelift` feature either, and there was one — on by
default, pulling five crates and a triple parser — until 2026-08-29.
CODEGEN-STENCIL.md §13 is why it went and what went with it.

**`backend-stencil` is on by default and adds no crate.** Its feature list is
empty: the copy-and-patch backend (CODEGEN-STENCIL.md) is written here, and what
it needs from outside is a host `cc`, which is the same tool `build/link.rs`
already shells out to in order to produce an artifact at all. What being on
*does* cost is build time. `cli/build.rs` generates about twenty-three thousand
C functions, compiles them with the host `cc` in twelve parallel shards, reads
the objects back and extracts one stencil per exported symbol — and does that
three times, once per target (CODEGEN-STENCIL.md §3, §3.2). That is an
install-time cost paid once per toolchain build rather than a cost inside the
loop, which is the same argument `libburi_rt.a` is built on, and it is the
reason the feature is worth having a name. It costs size too: the three
libraries are `include_bytes!`d, and they are 11.93 MB of the shipped `buri`
(PERFORMANCE.md §5).

It degrades rather than breaks, and what "degrades" means changed when this
backend took the debug seat. A host with no `cc`, or one with no library for its
target, gets an **empty** library; `stencil::AVAILABLE` reads the emptiness and
the backend reports itself unavailable, exactly as `runtime_native::AVAILABLE`
does for the archive. `actions::native_ready` is then false, `host_platform()`
answers `Js`, and a suite that names no platform is compiled and run as
JavaScript with the reason printed. That is a real degradation rather than a
no-op — it used to be a no-op, because `select` returned Cranelift and this
backend was never asked — and it is the same degradation a toolchain built
`--no-default-features` has always had.

**`backend-llvm` is off by default.** It needs LLVM 21 installed and
`LLVM_SYS_211_PREFIX` set (CODEGEN-LLVM.md §8). `cargo install buri` must not
require that, so it does not.

### 2.1 What a toolchain without LLVM does

It refuses `--release` for a native platform, with a diagnostic naming the
feature — it does not fall back to the debug backend. A `--release` build that silently
produced different code depending on how the compiler happened to be installed
is the wrong kind of surprise, and a refusal naming the feature is the right
one.

The hazard that follows is two `buri` binaries with identical sources and
different capabilities. **Nothing pins which one a repository is built with**:
`REPO.buri` used to name an exact toolchain version and its SHA-256, and that
pin was removed because a pin earns its keep only where a toolchain is fetched
and nothing fetches one (`buri docs build/hermeticity`). What stands in its
place is the refusal above — a toolchain that cannot do the job says so rather
than doing a different one — plus `Backend::identity()` (ARCHITECTURE.md §3),
which puts the LLVM version into every release `codegen` key, so two
LLVM-enabled toolchains built against different LLVMs do not share cache
entries.

### 2.2 The runtime archive

`cli/runtime` is a Rust static library with a C ABI (VALUE-MODEL.md §10), built
for the host by `cli/build.rs`:

```
<assemble $OUT_DIR/rt-pkg from cli/runtime/: manifest.toml -> Cargo.toml,
                                             manifest.lock -> Cargo.lock,
                                             the .rs files beside them>
cargo fetch --locked --offline --manifest-path $OUT_DIR/rt-pkg/Cargo.toml
cargo rustc --release --lib --locked \
      --manifest-path $OUT_DIR/rt-pkg/Cargo.toml \
      --target <host triple> --target-dir $OUT_DIR/rt \
      -- --remap-path-prefix==./runtime -Cmetadata=buri_rt -Cextra-filename=
```

and `include_bytes!`-ed into the binary through
`backend::runtime_native::ARCHIVE`. `cargo` and `rustc` are already required to
build the toolchain, so this adds no tool; it adds nothing to the *toolchain's*
lockfile, because the runtime has one of its own; and `--target <host triple>`
names the one triple this build supports, which is why cross-compilation is
refused rather than half-working (ARCHITECTURE.md §9).

Three things about that shape are decisions rather than mechanics, and
`cli/build.rs`'s header argues each in full:

- **The package is assembled in `OUT_DIR` rather than checked in**, and its
  manifest is `manifest.toml`. `cargo package` skips any subdirectory of a
  package that holds a `Cargo.toml` — unconditionally, ahead of
  `include`/`exclude` — so a manifest in `cli/runtime/` deletes the whole
  directory from the published `buri` crate, and a `cargo install buri` from a
  registry tarball then fails in the build script with no runtime to compile.
  That regression is what this repairs; the assertion is
  `.github/scripts/assert-package-ships-runtime.sh`.
- **`cargo rustc --`, not `RUSTFLAGS`.** The three flags belong to the runtime
  crate alone. Applied to the whole tree, an empty `-C extra-filename=` is a
  collision the moment two versions of one crate appear — and two do:
  `getrandom` 0.2 and 0.3 are both in the runtime's lockfile.
- **A tree that cannot be *resolved* degrades; one that cannot be *compiled*
  does not.** `cargo fetch --locked`, offline first, is the probe. It answers
  the plane, the sandbox, and the stale lockfile with an empty archive, a
  `cargo:warning` naming which, and a toolchain that still builds; a crate that
  resolved and then failed to compile fails the build, because that is a broken
  runtime rather than a missing one.

The three settings past the obvious ones each fix something measured rather than
guessed, on `aarch64-apple-darwin`. Two of them now live in the runtime's
`[profile.release]`, where a reader looks for them, and the third stays on the
command line because Cargo has no profile key for it:

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

  **The policy, ruled on and stated in full at CODEGEN-LLVM.md §8.1:** there is
  **exactly one** supported LLVM at any moment, and the pin is the latest that
  inkwell and this flake both carry — which is why this line and `cli/Cargo.toml`'s
  `llvm21-1` are one decision written twice, and why they may never disagree.
  Multi-version support is refused permanently; a contributor who wants a
  different LLVM gets it by not using `nix develop`, and then owns the mismatch.
  Bumping is a **routine chore**, not a compatibility event: bump this
  `llvmPackages_N`, the inkwell feature, `LLVM_SYS_<N>1_PREFIX`, and
  `backend/llvm/attrs.rs`'s location list, then let
  `the_bitmask_matches_llvm_21s_location_list` catch the one of those four that
  fails silently. No deprecation window, because there is no second version to
  deprecate. The LLVM version is an internal detail — nothing a program, a BUILD
  file or a diagnostic names — with `Backend::identity()` the sole exception, and
  that is a cache key rather than an interface.
- **`llvm.dev`**, not `llvm`. The `.dev` output carries `bin/llvm-config` and the
  headers; the default output does not, and the failure does not say so.
- **`mkShell` rather than `mkShellNoCC`.** `llvm-sys`'s build script needs a C++
  compiler, and the link step shells out to `cc` (CODEGEN-STENCIL.md §12.3), as
  does `cli/build.rs` for the stencil library (§2). This is a change to the
  existing shell, which is `mkShellNoCC` today.
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

`.github/workflows/ci.yml` runs on push, on pull request and on demand, and it
is eight jobs. The three that this document is about:

- **`test`** — the whole Rust suite, on every host this toolchain supports:
  `macos-latest` (arm64), `ubuntu-24.04` (x86_64) and `ubuntu-24.04-arm`. Each
  leg sets `CC: clang` and therefore builds its own stencil library, which
  `.github/scripts/assert-stencils.sh` then holds to being non-empty. There is
  deliberately no leg without `clang`: it would be the same suite with the
  native tests silently skipped, and that is the one shape of green this
  workflow exists to refuse.
  `.github/scripts/assert-runtime-archive.sh` is the same gate for the runtime
  archive — non-empty, under a per-OS size budget, and carrying no symbol from
  any of §1.1.1's four crates — and it runs on all four native jobs, `release`
  included.
- **`minimal`** — `cargo build -p buri --no-default-features` on
  `ubuntu-latest`, plus
  `.github/scripts/assert-package-ships-runtime.sh`, which is the other half of
  "`cargo install buri` works": the tarball has to carry what `cli/build.rs`
  compiles. It is the test that `cargo install buri` still works on a
  machine carrying no LLVM, and it is a job rather than an assertion because
  "it builds without the optional system library" is only true if something
  builds it that way. The default-feature build needs no job of its own: every
  `test` leg is one.
- **`release`** — the only leg that turns `backend-llvm` on. x86-64 only, and
  **advisory** (`continue-on-error: true`). The reason is named in the workflow
  rather than left to be discovered: LLVM 21 is not in Ubuntu 24.04 — noble
  ships 18 — so the job depends on apt.llvm.org, which is third-party
  infrastructure with no uptime guarantee, and a red X caused by a 404 teaches
  people to ignore a red X. arm64 is not attempted, because apt.llvm.org's
  architecture coverage per release is not something to discover inside a
  required job. The version is asserted rather than assumed, which is §8.1's
  one-supported-LLVM policy enforced where it can be.

Two further native jobs, `linux-arm64` and `linux-x86_64`, run the artifacts
rather than only compiling them, and CODEGEN-STENCIL.md §10 is where they are
described. `lean`, `tree-sitter` and `nix` complete the eight.

The cross-backend agreement differential test is not a CI feature:
`cli/tests/native/agreement.rs`
runs in the ordinary suite on every leg (ARCHITECTURE.md §4).

### 3.4 Without nix

**The default build needs no library and no system package.** `cargo build -p
buri`, `cargo test -p buri` and `cargo install buri` work on a machine with a
Rust toolchain and a C compiler and no LLVM, no lld and no mold anywhere on it —
and, since 2026-08-29, with nothing in the dependency closure either (§1.1).
That is the whole point of §2's default set, and it is the state a contributor
is in unless they go looking for the other one.

`cc` is the one thing that is not optional, and it is not new: the link step
drives the platform C compiler (CODEGEN-STENCIL.md §12.3) and
`cli/tests/native/runtime.rs` compiled a C driver against the runtime archive
from the first native wave. It is Xcode's command-line tools on macOS
(`xcode-select --install`) and `build-essential` on Debian-likes. `cli/build.rs`
also uses it to generate the stencil library (§2), and that is the one place the
requirement got sharper: a host without `cc` still builds a `buri`, and gets an
empty library, a backend that reports itself unavailable, and JavaScript for
every suite that does not name a platform.

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
`run_on` (`test.rs`) computes `test_key`, consults `Cache`, and returns
cached results with `Provenance::Cache` when the key hits; the summary already
prints "`n` cached" (`test.rs`). Only a clean run is cached, deliberately
— "a failure is what you are trying to fix, and re-running it should re-run it"
(`test.rs`) — which is exactly the behaviour a watch loop wants.

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
  `testing/` sources (`actions.rs`);
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
(`session.rs`) and a `Session` holds a loaded graph rather than a
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
  (`test.rs`), so every keystroke would re-run every suite in the selection.
  That is the opposite of the mode.
- **`--watch --accept`.** `--accept` is "the one mode that writes to the source
  tree" (`test.rs`). A mode that rewrites golden files on a timer is a
  mode that silently accepts a regression while you are reading the failure.
- **`--watch` without a TTY.** A watch loop in CI is a hung job. The check is on
  stdout being a terminal, and the diagnostic says so and names `buri test`.

### 4.4 The terminal

- **The screen is not cleared.** Scrollback is where the failure you are fixing
  is; the previous run's output is the thing you are comparing against. Each run
  is separated by a rule with the time on it, which is one line and is greppable:
  `── 14:02:31 ─────`.
- **One summary line per run**, in the format `cmd_test` already prints
  (`test.rs`): `12 passed, 1 failed, 0 skipped (0.4s, 8 cached)`. Failures
  print above it exactly as they do without `--watch`, through `report_failure`,
  so a suite's output does not depend on which mode it was run in.
- **A run with nothing to do prints nothing at all.** If every suite was served
  from the cache and every one passed, the loop is silent. A watch mode that
  prints on every sweep trains you not to read it.
- **`--explain` works**, and in watch mode it is the most useful it has ever been:
  one `test` line per suite per run with `cached` or `run`, which is the
  incrementality claim being observable rather than asserted
  (`arguments.rs`).
- **Ctrl-C exits 0**, whatever the last run said. The exit status of a watch loop
  is about the loop; a red suite is on the screen, and encoding it in `$?` would
  make `buri test --watch` unusable in a shell with a prompt that shows the last
  status.

### 4.5 What it is not

Not a hot-reload, not a REPL, and not a language server. It re-runs `buri test`
and it does not keep a compiled program alive between runs. The reason to say so
is that the machinery that would make it more — a persistent process holding the
checked standard library between runs — is the same machinery
`design/TODO.md` names as still missing under "Incrementality and caching"
("nothing shares work between processes"), and it is a separate, larger piece
of work whose first customer
would be the language server rather than this.

## 5. Implementation waves

The waves have landed. The collision map that made them safe to run in parallel
is not kept — it described who was allowed to write which file during a rollout
that is over, and what it produced is the module layout in
ARCHITECTURE.md §2 and the action graph in ARCHITECTURE.md §6.

What is kept is the **legend**, because the wave labels are still module headers
in the source (`//! ... **Wave 2b.**`) and a reader who meets one needs
somewhere to look it up. It is one table for the whole corpus rather than one
per document, so it lives at
[`design/README.md`](../README.md), under "Wave numbering", together with the
one piece of wave 3c that did not land.

### What is not in any wave

DWARF (CODEGEN-LLVM.md §7 sketches it, CODEGEN-STENCIL.md §11 declines it for
the debug backend),
cross-*linking* (ARCHITECTURE.md §9 refuses it), ThinLTO (ARCHITECTURE.md §5.2
leaves the door open), general niche discovery (VALUE-MODEL.md §6), and
small-string optimization (VALUE-MODEL.md §3.2). Each is named where it is
declined so that a later reader finds the reason next to the decision rather than
in a plan.
