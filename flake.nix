{
  description = "compiler toolchain";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
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
          ];
        };
      }
    );
}
