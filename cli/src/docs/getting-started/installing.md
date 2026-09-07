# Installing

There is no release yet, so every path below builds from source. They produce
the same binary and differ only in where the Rust toolchain comes from.

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

`--HEAD` builds the `main` branch. You need it until a release is tagged.

**Cargo**, with a Rust toolchain already in hand:

```sh
cargo install --locked --path cli
```

The binary has no runtime dependencies. Linking a native binary uses the system
C toolchain: `cc`, or whatever `CC` names. The JavaScript path looks for `bun`
or `node` on your `PATH`, or the one `BURI_JS` names.

## Setting up a repository

Your first repository is one command:

```sh
buri init hello-buri && cd hello-buri && buri test //...
buri init                                # into the working directory instead
```

`buri init` writes a repository that builds, tests, lints and formats clean the
moment it lands:

| What lands | What it is |
|---|---|
| `REPO.buri` | The repository root, with linting on from the first commit |
| `libs/greeting/` | A library, in the two files a library needs: `lib.buri` and one module behind it |
| `libs/greeting/test/` | Its test suite, importing the library by label the way a dependent does |
| `apps/hello/` | A binary that depends on the library, with `main` and its context |
| `.gitignore` | What the build writes |
| `.agent/skills/` | The agent skills, one directory per skill |

Run `init` rather than copying files out of a page: those skills are the
release's own account of the toolchain, and each is a page here too, starting
with [the language skill](../reference/skills/buri-language.md).

`buri init` never writes over your work. A `REPO.buri` at the target, or above
it, stops the command — a repository inside another one is not a root. A
`.gitignore` already at the target is the one exception, since `git init` first
is the ordinary way to start; there `buri init` appends its entries below your
lines.

## Skills in a repository you already have

```sh
buri add skills                            # here
buri add skills ~/src/some-other-repository
```

`buri add skills` writes the same skills into `.agent/skills/` and touches
nothing else. The skills are compiled into the binary, so it works in any
directory, Buri repository or not.

Run it again to upgrade. A skill directory whose name begins `buri-` belongs to
the toolchain, so every run rewrites it and removes any the release has stopped
shipping. A directory named anything else is yours, and `buri add skills` never
reads, writes, or removes it.

## Next

[Your first program](./first-program.md) comes next.
