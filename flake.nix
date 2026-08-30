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
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          inherit (cargoToml.package) version;
          # The flake's own source. In a checkout that is a git repository this
          # is the tracked tree, so a `nix build` that fails on a file `cargo
          # build` finds is a file that has not been `git add`ed yet.
          src = self;

          # The toolchain's dependencies are held to the bar stated in the root
          # `Cargo.toml`: a code generator or a platform interface, behind a
          # cargo feature the default build can turn off, whose absence degrades
          # rather than breaks. One crate has cleared it -- `inkwell` behind
          # `backend-llvm` -- so the lockfile names its closure and vendoring
          # fetches it. `cargoLock.lockFile` reads the hashes out of the
          # lockfile, so there is still no `cargoHash` to keep in sync by hand.
          #
          # **This vendors the toolchain's lockfile and not the runtime's, and
          # that is a known gap rather than an oversight.** Since 2026-08-30
          # `cli/runtime` has a dependency tree of its own — four crates behind
          # `net`, `cli/runtime/manifest.lock` — and `cli/build.rs` runs a
          # nested `cargo` to build it. In this sandbox that cargo can reach
          # neither the network nor a vendor directory holding those crates, so
          # it degrades exactly as it is designed to: an empty archive, a
          # `cargo:warning` saying so, `runtime_native::AVAILABLE == false`, and
          # a `buri` that builds and runs the JavaScript backend with no native
          # one. `nix build` is therefore still green and still produces a
          # *less capable* toolchain than a `cargo install` does, which wants
          # closing before the flake is the recommended way to get `buri`.
          # Closing it means vendoring both lockfiles into one directory the
          # nested cargo can see through `CARGO_HOME`; `importCargoLock` takes
          # one `lockFile`, so it is a merge and a piece of work of its own.
          cargoLock.lockFile = ./Cargo.lock;

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
