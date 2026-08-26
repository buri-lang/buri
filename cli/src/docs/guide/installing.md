## Installing

There is no release yet, so every path below builds from source. They produce
the same binary and differ only in what supplies the Rust toolchain.

**Nix.** This repository is a flake, and its default package is `buri`:

```sh
nix run github:buri-lang/buri -- version   # run it once, install nothing
nix profile install github:buri-lang/buri  # keep it
```

**Homebrew.** This repository is also its own tap:

```sh
brew tap buri-lang/buri https://github.com/buri-lang/buri.git
brew install --HEAD buri-lang/buri/buri
```

`--HEAD` builds the `main` branch and is required until a release is tagged;
after that, drop it.

**Cargo**, with a Rust toolchain already in hand:

```sh
cargo install --locked --path cli
```

Your first repository is one command. `buri init` writes a working library, a
binary that depends on it, and a test suite, and installs the agent skills:

```sh
buri init hello-buri && cd hello-buri && buri test //...
```

The binary carries no runtime dependencies. Linking a native binary uses the
system C toolchain (`cc`, or whatever `CC` names); the JavaScript path resolves
a runtime — `bun` or `node` — from `PATH`, or from `BURI_JS` naming one.
