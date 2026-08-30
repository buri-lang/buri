{
  description = "compiler toolchain";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    # The four systems a `buri` binary is expected on: aarch64 and x86_64,
    # Darwin and Linux.
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

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
        toolchainCrates = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
        runtimeCrates = pkgs.rustPlatform.importCargoLock {
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
        packages.default = pkgs.rustPlatform.buildRustPackage {
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
          # `.github/scripts/assert-runtime-archive.sh` is the repository's own
          # liveness gate, run here rather than reimplemented: non-empty, under
          # the per-OS size budget, and carrying no symbol from any of the
          # runtime's four networking crates. It runs on the four native CI jobs
          # already; the reason it also runs *here* is that the failure it
          # catches is exactly the one this flake had — `cli/build.rs` degrades
          # to an empty archive rather than breaking, so a vendoring mistake
          # produces a green `nix build` and a toolchain with no native backend,
          # which is invisible until a user's `buri build` refuses.
          #
          # An assertion and not a degradation, because on **these** hosts there
          # is nothing to degrade to: `flake-utils.lib.eachDefaultSystem` builds
          # for aarch64/x86_64 × Darwin/Linux, and `cli/build.rs`'s `supported`
          # is `-apple-darwin` or `-linux-`, so every system this derivation is
          # ever instantiated for is one the runtime builds on. The genuinely
          # unsupported host still gets its empty archive; it just does not get
          # it from here.
          postBuild = ''
            bash .github/scripts/assert-runtime-archive.sh target
          '';

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
        };

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
