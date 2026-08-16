{
  description = "compiler toolchain";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
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
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          inherit (cargoToml.package) version;
          # The flake's own source. In a checkout that is a git repository this
          # is the tracked tree, so a `nix build` that fails on a file `cargo
          # build` finds is a file that has not been `git add`ed yet.
          src = self;

          # The toolchain has no dependencies on purpose (see the root
          # `Cargo.toml`), so the lockfile names one package -- this crate --
          # and vendoring fetches nothing. There is no `cargoHash` to keep in
          # sync because there is nothing to hash.
          cargoLock.lockFile = ./Cargo.lock;

          # `cargo test` compiles and *runs* the examples in `SPEC.md` and
          # `README.md`, which means spawning a JavaScript runtime -- a package
          # build must not depend on that, so the suite stays in `nix develop`.
          doCheck = false;

          # No runtime dependency on a JavaScript runtime: `bun` is a
          # development tool, not something an install should carry. `buri run`
          # and `buri test` resolve a runtime from the user's own `PATH` (or
          # `BURI_JS`) when they are used -- cli/src/build/spawn.rs.

          meta = {
            inherit (cargoToml.package) description;
            # No `homepage`. `nix run github:<owner>/<repo>` names the
            # repository at the call site, so a flake never has to know where
            # it is hosted -- and the one place that does, `Formula/buri.rb`,
            # is then the only place a move has to be reflected.
            mainProgram = "buri";
            # No `license` until the repository has a LICENSE file; a license
            # asserted here would be an assertion nothing backs.
            platforms = pkgs.lib.platforms.unix;
          };
        };

        # `nix run github:<owner>/<repo> -- version`.
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/buri";
          meta = { inherit (cargoToml.package) description; };
        };

        devShells.default = pkgs.mkShellNoCC {
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
          ];
        };
      }
    );
}
