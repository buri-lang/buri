## Installing

There is no release yet, so every path below builds from source. They produce
the same binary and differ only in what supplies the Rust toolchain.

**Nix.** This repository is a flake, and its default package is `buri`:

```sh
nix run github:<owner>/buri -- version   # run it once, install nothing
nix profile install github:<owner>/buri  # keep it
```

**Homebrew.** This repository is also its own tap:

```sh
brew tap <owner>/buri https://github.com/<owner>/buri.git
brew install --HEAD <owner>/buri/buri
```

`--HEAD` builds the `main` branch and is required until a release is tagged;
after that, drop it.

**Cargo**, with a Rust toolchain already in hand:

```sh
cargo install --locked --path cli
```

Whichever path installed it, the binary carries no runtime dependencies.
`buri run` and `buri test` are the two commands that execute what they compile.
`buri test` runs a suite as a native binary for the host and needs a C
toolchain to link it — `cc`, or whatever `CC` names — which every developer
machine already has. What it falls back to, and what `buri run` executes for a
binary that declares no output, is JavaScript: that path resolves a runtime from
`PATH`, or from `BURI_JS` naming one, and `bun` is what the toolchain's own
development uses.
