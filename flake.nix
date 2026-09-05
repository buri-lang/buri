{
  description = "compiler toolchain";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";

    # THE THIRD INPUT, AND WHAT IT BUYS: A LINUX PACKAGE THAT CAN LINK.
    #
    # This flake had two inputs on purpose, and an input is a real cost — a
    # second source of a compiler, a second lockfile entry, a second thing to
    # bump. What buys this one is not convenience: it is the difference
    # between a `buri` that produces Linux executables and one that refuses
    # to.
    #
    # `pkgs.rustPlatform`'s rustc ships `rust-std` for the host triple alone,
    # and there is no `rustup` in a sandbox to add another with. On Linux
    # `cli/build.rs` needs `<arch>-unknown-linux-musl`'s std — it probes
    # `rustc --print target-libdir --target <musl>` for the `self-contained/`
    # directory it bakes eleven files out of — so without it the derivation
    # took `build.rs`'s documented degradation and built a **glibc** archive
    # with an empty sysroot. The toolchain that came out then refused every
    # native Linux link in the product's own words
    # (`build/link.rs::libc_for`): loud, correct, and a compiler with its
    # native backend switched off by the way it was packaged.
    #
    # `rust-bin.stable.latest.default.override { targets = [ ... ]; }` is
    # upstream's own dist tarball, which is exactly where the
    # `self-contained/` directory comes from — measured, not assumed: the
    # override's sysroot holds precisely the eleven files `cli/src/build/musl.rs`
    # bakes, and `--print target-libdir --target <musl>` resolves into it.
    # `pkgs.pkgsCross.musl64` was the other candidate and answers a different
    # question — it cross-compiles the *toolchain* to musl, where what is
    # wanted is a toolchain that can produce musl artifacts — and a nixpkgs
    # std built against its own musl carries no `self-contained/` at all.
    #
    # `follows`, so that the overlay's nixpkgs is this flake's nixpkgs: one
    # copy of nixpkgs in the lockfile, and a `pkgs` whose rust and whose
    # everything-else come from the same tree.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, flake-utils, rust-overlay }:
    # The four systems a `buri` binary is expected on: aarch64 and x86_64,
    # Darwin and Linux.
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        # The overlay is what puts `pkgs.rust-bin` there; nothing else in this
        # `pkgs` changes, so every other package is still the one `nixos-25.11`
        # names.
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        inherit (pkgs) lib;

        # The triple the Linux half of this package is *about*.
        #
        # A substring replacement over the host's own config triple, which is
        # the same transformation `cli/build.rs::product_target` makes and is
        # deliberately the same shape: an `arm-unknown-linux-gnueabihf` host
        # becomes `...-musleabihf`, where a triple rebuilt from the
        # architecture would have dropped the ABI suffix and named a target
        # that does not exist. On Darwin the replacement finds nothing, the
        # value is the host triple, and nothing below reads it.
        muslTarget =
          builtins.replaceStrings [ "-linux-gnu" ] [ "-linux-musl" ]
            pkgs.stdenv.hostPlatform.config;
        # `cc-rs` spells its per-target variables with underscores:
        # `CC_aarch64_unknown_linux_musl`. Same string, one substitution.
        muslKey = builtins.replaceStrings [ "-" ] [ "_" ] muslTarget;

        # The compiler this package is built by — the overlay's on Linux, and
        # nixpkgs' own everywhere else.
        #
        # **The split is deliberate and it is measured.** The overlay is here
        # for one thing, a musl `rust-std`, and macOS has no use for one: a
        # `buri` on Darwin produces Mach-O. Taking the overlay there anyway
        # would not be free and would not be neutral — it is a *compiler
        # version bump* smuggled in beside a libc fix, because
        # `rust-bin.stable.latest` is 1.98.0 where this flake's `nixos-25.11`
        # pins 1.91.1. What that costs was not guessed: built on aarch64-darwin,
        # 1.98.0's runtime archive is 9 582 112 bytes against the 9 437 184-byte
        # Darwin budget in `cli/tests/ci.rs`, so the
        # bump alone turns `nix build` red on macOS — an artifact-size decision
        # arriving as a side effect of a Linux packaging one, which is exactly
        # the shape of change this repository argues against. Linux takes the
        # newer compiler because there it buys the musl target; Darwin keeps
        # the one nixpkgs pins, and every input to its derivation is the one
        # this change found — `src` aside, which every edit to this file moves.
        #
        # `makeRustPlatform`, because `buildRustPackage` takes its `rustc` and
        # `cargo` from a platform rather than from the arguments, and a
        # toolchain wired anywhere else is a toolchain the build does not use.
        # `rustToolchain` is never forced on Darwin: nix is lazy, and the `if`
        # below is what keeps the dist tarball from being fetched there.
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ muslTarget ];
        };
        rustPlatform =
          if pkgs.stdenv.hostPlatform.isLinux then
            pkgs.makeRustPlatform {
              cargo = rustToolchain;
              rustc = rustToolchain;
            }
          else
            pkgs.rustPlatform;

        # The version is read rather than written down. `buri version` prints
        # `CARGO_PKG_VERSION`, so a version repeated here is a second place to
        # forget, and a package that claims 0.3.0 while its binary says 0.4.0
        # is worse than one that claims nothing.
        cargoToml = builtins.fromTOML (builtins.readFile ./cli/Cargo.toml);

        # LLVM 21.1, pinned deliberately rather than taken from the default
        # `llvmPackages` (design/native/CODEGEN-LLVM.md §8). The flake's
        # `nixos-25.11` provides 12 through 21 (21.1.7 is the default) and no
        # 22 -- so pinning 22, which is the newest inkwell supports, would mean
        # bumping this flake's nixpkgs in service of a codegen decision.
        #
        # `.dev`, not the default output: `.dev` carries `bin/llvm-config` and
        # the headers, which is what `llvm-sys`'s build script looks for, and
        # pointing at the default output fails in a way whose error message
        # does not say so.
        llvm = pkgs.llvmPackages_21.llvm;

        # ---------------------------------------------------------------------
        # Two lockfiles, one vendor directory.
        # ---------------------------------------------------------------------
        #
        # This repository has **two** cargo dependency trees and a sandboxed
        # build has to carry both. The toolchain's is `./Cargo.lock`. The
        # runtime's is `cli/runtime/manifest.lock` — `cli/runtime` is a package
        # of its own, four crates behind `net`, and `cli/build.rs` runs a
        # *nested* `cargo` to build it into `libburi_rt.a`, the archive every
        # native binary this compiler produces is linked against.
        #
        # `rustPlatform.importCargoLock` takes one `lockFile`, so vendoring the
        # toolchain's alone left that nested cargo with no source for the
        # runtime's tree: it took the degradation path `cli/build.rs`'s header
        # argues for — an empty archive, a `cargo:warning`,
        # `runtime_native::AVAILABLE == false` — and `nix build` produced a
        # green toolchain *less capable* than the one `cargo install buri`
        # produces. A packaging path that silently drops the native backend is
        # not a packaging detail; it is a different compiler under the same
        # version number, and it is what this merge closes.
        #
        # **The merge is of the directories, not of the lockfiles**, and that is
        # a decision rather than a convenience. Cargo's vendored `directory`
        # source is a flat set of `name-version` directories, so two vendor
        # directories become one by linking both sets into a third and writing
        # the source-replacement config once. Merging the lockfiles instead
        # fails on nixpkgs' own hook: `cargoSetupPostPatchHook` diffs the vendor
        # directory's `Cargo.lock` against the one in `src` and aborts the build
        # when they differ, so a merged lockfile would have to be un-merged
        # again before the build could start. Here `Cargo.lock` stays the
        # toolchain's, byte for byte.
        #
        # Neither `importCargoLock` call takes a hash: both read theirs out of
        # the lockfile they are given, so there is still no `cargoHash` to keep
        # in sync by hand and a lockfile edit needs no second edit here.
        toolchainCrates = rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
        runtimeCrates = rustPlatform.importCargoLock {
          lockFile = ./cli/runtime/manifest.lock;
        };

        # **The name is load-bearing.** `cargoSetupHook` copies `$cargoDeps`
        # into the build root under its own basename with the store hash
        # stripped, and the `directory =` line below names that basename — so a
        # derivation called anything else produces a config pointing at a
        # directory that is not there.
        cargoVendorDir = pkgs.runCommand "cargo-vendor-dir" { } ''
          mkdir -p $out/.cargo

          # The toolchain's, unmerged: this is the file the hook diffs against
          # `src`'s, and the two must be identical.
          ln -s ${./Cargo.lock} $out/Cargo.lock

          cat > $out/.cargo/config.toml <<'EOF'
          [source.crates-io]
          replace-with = "vendored-sources"

          [source.vendored-sources]
          directory = "cargo-vendor-dir"
          EOF

          # Deduplicated by `name-version`, and that is exact rather than
          # hopeful: crates.io fixes the checksum for a name and a version, and
          # `importCargoLock` fetches by that checksum, so a crate in both trees
          # is the same bytes from the same store path and the second link would
          # only fail with "File exists".
          #
          # A git dependency's crate directory carries a `.cargo-config` stanza
          # that has to reach the config file above, keyed by source URL rather
          # than by crate because one repository can hold several. Neither
          # lockfile has a git dependency today; the loop carries them anyway,
          # because the alternative is a trap that springs on whoever adds the
          # first one and presents as an unresolvable tree rather than as an
          # error about vendoring.
          declare -A seen
          for crate in ${toolchainCrates}/*/ ${runtimeCrates}/*/; do
            crate=''${crate%/}
            name=$(basename "$crate")
            if [ -n "''${seen[crate:$name]:-}" ]; then continue; fi
            seen[crate:$name]=1
            ln -s "$crate" "$out/$name"

            if [ -e "$crate/.cargo-config" ]; then
              key=$(sed 's/\[source\."\(.*\)"\]/\1/; t; d' < "$crate/.cargo-config")
              if [ -z "''${seen[source:$key]:-}" ]; then
                seen[source:$key]=1
                cat "$crate/.cargo-config" >> $out/.cargo/config.toml
              fi
            fi
          done
        '';
      in
      {
        packages.default = rustPlatform.buildRustPackage ({
          pname = cargoToml.package.name;
          inherit (cargoToml.package) version;
          # The flake's own source. In a checkout that is a git repository this
          # is the tracked tree, so a `nix build` that fails on a file `cargo
          # build` finds is a file that has not been `git add`ed yet.
          src = self;

          # The dependencies of both trees are held to the bar stated in the
          # root `Cargo.toml`: a code generator or a platform interface, behind
          # a cargo feature the default build can turn off, whose absence
          # degrades rather than breaks. One crate has cleared it on the
          # toolchain's side -- `inkwell` behind `backend-llvm` -- and four on
          # the runtime's, behind `net`; the lockfiles name their closures and
          # `cargoVendorDir` above fetches both.
          #
          # `cargoDeps`, not `cargoLock`. `cargoLock` is sugar for one
          # `importCargoLock` over one lockfile, which is the one thing this
          # package cannot use: the nested cargo in `cli/build.rs` needs the
          # runtime's tree in the same vendor directory. Nothing else about the
          # vendoring changes -- no `cargoHash`, and the sources are still
          # fetched by the checksums in the lockfiles.
          cargoDeps = cargoVendorDir;

          # Default features, which is `backend-stencil` alone -- and it needs
          # no crate, so the default build fetches nothing.
          #
          # design/native/BUILD-AND-WATCH.md §3.2 wants this built **with**
          # `backend-llvm`, because a `nix build` produces the release
          # toolchain and a release toolchain must be able to produce release
          # artifacts. That flip is three lines -- `buildFeatures = [
          # "backend-llvm" ]`, `nativeBuildInputs = [ llvm.dev ]`, and
          # `LLVM_SYS_211_PREFIX` -- and it is deliberately not taken in the
          # same change as the dependency itself, because it cannot be checked
          # from a working tree: `src = self` is the *tracked* tree, so a
          # `nix build` run beside an uncommitted backend builds the previous
          # one and proves nothing about the new. It lands with the commit that
          # makes the LLVM backend part of the tracked tree, where `nix build`
          # is a real test of it rather than a claim.
          buildNoDefaultFeatures = false;

          # The archive is real, and this build fails if it is not.
          #
          # The same liveness gate `cli/tests/ci.rs::the_runtime_archive_is_real`
          # is, reduced here to the two halves this sandbox can ask: the archive
          # is not empty, and on Linux it is a musl archive. The suite's copy is
          # the fuller one — size budget, symbol table, the entropy door — and it
          # cannot run here, because this derivation builds the toolchain and
          # does not run its tests. The reason a copy exists at all is that the
          # failure it catches is exactly the one this flake had: `cli/build.rs`
          # degrades to an empty archive rather than breaking, so a vendoring
          # mistake produces a green `nix build` and a toolchain with no native
          # backend, which is invisible until a user's `buri build` refuses.
          #
          # An assertion and not a degradation, because on **these** hosts there
          # is nothing to degrade to: `flake-utils.lib.eachDefaultSystem` builds
          # for aarch64/x86_64 × Darwin/Linux, and `cli/build.rs`'s `supported`
          # is `-apple-darwin` or `-linux-`, so every system this derivation is
          # ever instantiated for is one the runtime builds on. The genuinely
          # unsupported host still gets its empty archive; it just does not get
          # it from here.
          postBuild = ''
            archive=$(find target -path '*/out/libburi_rt.a' -size +0 | head -1)
            if [ -z "$archive" ]; then
              echo "libburi_rt.a is empty or absent: this toolchain has no native backend" >&2
              exit 1
            fi
            if [ "$(uname -s)" = Linux ] && ! grep -qx musl "$archive.libc"; then
              echo "libburi_rt.a was built against $(cat "$archive.libc"), not musl" >&2
              exit 1
            fi
            echo "libburi_rt.a: $(wc -c < "$archive") bytes"
          '';

          # WHICH LIBC THE ARCHIVE IS BUILT AGAINST, AND WHY NOTHING HERE
          # RELAXES THE ANSWER ANY MORE.
          #
          # On a Linux host `cli/build.rs` builds the runtime archive for
          # `<arch>-unknown-linux-musl`, so that every artifact `buri build`
          # links is a static-PIE musl executable that runs on any Linux
          # (design/native/ARCHITECTURE.md §9, CODEGEN-STENCIL.md §12.3). It
          # finds the bytes for that by asking `rustc --print target-libdir
          # --target <musl>` for a `self-contained/` directory, and that
          # directory exists only where the musl `rust-std` is installed beside
          # the compiler. `rustToolchain` above is that compiler: the
          # `targets = [ muslTarget ]` override is what puts the directory
          # there, and the `postBuild` assertion above is what says so.
          #
          # This block used to set `BURI_ARCHIVE_LIBC_MAY_BE_GLIBC=1`, the one
          # escape hatch the runtime-archive assertion had, because
          # `pkgs.rustPlatform`'s rustc ships std for the host triple alone and
          # a sandbox has no `rustup` to add a target with. The archive was a
          # `gnu` archive, the baked sysroot was empty, and the toolchain that
          # came out refused every native Linux link. Nothing sets that variable
          # now — nothing has it any more — and the libc assertion is
          # load-bearing on `x86_64-linux` and `aarch64-linux` again: a
          # `nix build` that produced a glibc archive would go red.
          #
          # `BURI_MUSL=off` is deliberately NOT set. It is read by the produced
          # binary at link time rather than by this build, so setting it here
          # does nothing — and setting it *for the produced binary*, by
          # wrapping it, would trade a hermetic executable for a `buri build`
          # that silently links against the user's glibc. That is the precise
          # outcome this whole arrangement exists to prevent, and a package
          # manager is the last place to make it the default.

          # `cargo test` compiles and *runs* the examples under `cli/src/docs/`,
          # which means spawning a JavaScript runtime -- a package
          # build must not depend on that, so the suite stays in `nix develop`.
          doCheck = false;

          # No runtime dependency on a JavaScript runtime: `bun` is a
          # development tool, not something an install should carry. `buri test`
          # compiles a suite to a native binary and needs only `cc` to link it;
          # where it falls back, and for `buri run` on a binary that declares no
          # output, a runtime is resolved from the user's own `PATH` (or
          # `BURI_JS`) when it is used -- cli/src/build/spawn.rs.

          meta = {
            inherit (cargoToml.package) description;
            # No `homepage`. `nix run github:buri-lang/buri` names the
            # repository at the call site, so a flake never has to know where
            # it is hosted -- and the two places that do, `Formula/buri.rb` and
            # `cli/Cargo.toml`, are then the only places a move has to be
            # reflected.
            mainProgram = "buri";
            # The repository's own `LICENSE`, as nixpkgs spells it.
            license = pkgs.lib.licenses.mit;
            platforms = pkgs.lib.platforms.unix;
          };
        }
        # RING'S C, AND THE ONE COMPILER IN THIS SANDBOX THAT CAN AIM AT MUSL.
        #
        # The rust half above is not the whole answer. The runtime's `ring` —
        # `rustls`'s crypto provider, and the reason `https://` works — builds
        # C and assembly through `cc-rs`, and a musl *rust* target makes that a
        # musl *C* compile too. `cc-rs` left to itself goes looking for
        # `musl-gcc` by name, finds none, and the build ends at a header it
        # cannot open; `cli/build.rs::musl_cc_env`'s own fallback is Debian's
        # `/usr/include/<arch>-linux-musl` over a `cc` that takes `--target=`,
        # and a nix sandbox has neither — its `cc` is gcc, which rejects the
        # flag outright. So the answer is named here, where the store paths are
        # known, rather than probed there.
        #
        # `CC_<target>`/`CFLAGS_<target>` is the pair `cc-rs` consults *first*,
        # ahead of every guess it would otherwise make, and `musl_cc_env`
        # returns untouched when it finds `CC_<target>` already set — that
        # deference is the contract this relies on, and the comment on that
        # function is its other half. The two must stay spelled the same way:
        # `muslKey` is the underscored triple `cc-rs` looks for.
        #
        # `pkgs.clang` and not the stdenv's `cc`, because one clang binary
        # compiles for every target it was built with and gcc compiles for one.
        # The `-isystem` is musl's own headers out of the store, which is the
        # nix-shaped answer to the Debian path above: hermetic, versioned with
        # the rest of `pkgs`, and no `/usr` anywhere in it. It precedes the
        # wrapper's own libc `-isystem` — cc-wrapper appends `NIX_CFLAGS_COMPILE`
        # *after* the caller's arguments — so musl's `stdlib.h` is the one
        # found, not glibc's.
        #
        # `NIX_CC_WRAPPER_SUPPRESS_TARGET_WARNING`: the clang wrapper prints a
        # line per compilation unit when it is handed a `--target=` other than
        # its own, on the reasoning that it "is currently not designed with
        # multi-target compilers in mind". What the warning is about is the
        # wrapper's *link* flags, which point at glibc — and nothing here
        # links: `cc-rs` compiles objects, `rustc` links them against the
        # baked musl sysroot. Suppressed with the argument written down rather
        # than left to scroll past on every file.
        //
          lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            "CC_${muslKey}" = "${pkgs.clang}/bin/clang";
            "CFLAGS_${muslKey}" = "--target=${muslTarget} -isystem ${pkgs.musl.dev}/include";
            NIX_CC_WRAPPER_SUPPRESS_TARGET_WARNING = "1";

            # THE SAME CLANG AS `CC`, AND THE SECOND HALF OF A NATIVE BACKEND.
            #
            # `cli/build.rs` builds the stencil library with `CC` — two Linux
            # blobs, cross-compiled as `clang --target=<arch>-unknown-linux-musl
            # -nostdinc -isystem $(clang -print-resource-dir)/include`, which is
            # clang's own headers and no sysroot
            # (`backend/stencil/sources.rs::compile_flags`). gcc has no
            # `--target=` and no `-print-resource-dir`, so with the stdenv's
            # `cc` the probe in `sources.rs::can_build` fails, both blobs are
            # written empty, and `backend::select` answers "the linux backend is
            # not implemented" for every native output — measured on this
            # derivation before this line existed, with a musl runtime archive
            # sitting right beside the empty stencils.
            #
            # `preBuild` and not an attribute, because cc-wrapper's setup hook
            # exports `CC` unconditionally during `setupPhase` and would
            # overwrite one. Only `CC` moves: `CXX` stays the stdenv's, and
            # nothing in either dependency tree compiles C++ under the default
            # features.
            preBuild = ''
              export CC=${pkgs.clang}/bin/clang
            '';
          }
        );

        # `nix run github:buri-lang/buri -- version`.
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/buri";
          meta = { inherit (cargoToml.package) description; };
        };

        # `mkShell`, not `mkShellNoCC`. Two things in the native branch need a
        # C toolchain and neither is optional: `llvm-sys`'s build script wants
        # a C++ compiler, and the link step drives `cc` because the C driver is
        # what knows where `crt1.o`, `libc` and `libSystem.tbd` live
        # (`cli/src/build/link.rs`).
        devShells.default = pkgs.mkShell {
          # `llvm-sys` refuses to guess. Without this the `backend-llvm` build
          # fails at its build script rather than at a link.
          LLVM_SYS_211_PREFIX = "${llvm.dev}";

          packages = [
              pkgs.cargo
              pkgs.bun
              # `elan`, not `lean4`: elan honours `formal/lean-toolchain`, which
              # is how a Lean project pins its compiler. It fetches that
              # toolchain on first use, so the Lean shell is not hermetic the
              # way the Rust one is -- acceptable only because nothing in
              # `formal/` is on the path to building a `buri` binary. See
              # formal/README.md.
              pkgs.elan
              # For building protobuf's `conformance_test_runner`, which
              # `cli/tests/proto/run.sh` drives and which nixpkgs does not
              # package -- `pkgs.protobuf` is the library and `protoc`, and the
              # runner is a test binary the release does not install. These are
              # what building one from the protobuf source needs;
              # cli/tests/proto/README.md has the recipe. Development only: the
              # suite is not part of `cargo test`.
              pkgs.cmake
              pkgs.ninja
              pkgs.abseil-cpp
              pkgs.zlib
              pkgs.pkg-config
              # -- the native backends -------------------------------------
              #
              # Nothing here is needed for `backend-stencil`, which is the
              # default and depends on no crate: `cargo build` works in a
              # shell with none of it. These are for `--features backend-llvm`
              # and for the link step, and they are in the shell rather than
              # in a `nix-shell -p` incantation because a contributor who
              # cannot build both backends cannot check that they agree.
              llvm.dev
              llvm
              # `llvm-config --system-libs` asks for these on most
              # configurations. `zlib` is already above; `zstd` and `ncurses`
              # are added if a configuration turns out to want them, which
              # varies.
              pkgs.libxml2
              pkgs.libffi
              # `ld64.lld` on macOS and `ld.lld` on Linux. It follows the
              # default `llvmPackages` rather than the pinned 21, which is
              # fine: a linker's version need not match the compiler's.
              pkgs.lld
          ]
          # mold is ELF-only -- it fails with "mold does not support macOS",
          # and the Mach-O fork was archived in November 2024 with its author
          # recommending Apple's linker (BUILD-AND-WATCH.md §3).
          ++ pkgs.lib.optional pkgs.stdenv.isLinux pkgs.mold;
        };
      }
    );
}
