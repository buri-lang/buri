# Installing

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

The binary carries no runtime dependencies. Linking a native binary uses the
system C toolchain (`cc`, or whatever `CC` names); the JavaScript path resolves
a runtime — `bun` or `node` — from `PATH`, or from `BURI_JS` naming one.

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

That last row is the reason to run `init` rather than copy files out of a page: a coding agent working in the repository gets the toolchain's own
account of the language, the type system, the build system, testing, and the
CLI, written by the release that is installed. Each is also a page here,
starting with [the language skill](../reference/skills/buri-language.md).

`buri init` never writes over your work. A `REPO.buri` at the target means the
directory is already a repository and the command stops; a `REPO.buri` *above*
the target stops it too, because a repository inside another one is not a root,
it is a stray build file in somebody else's tree.

## Skills in a repository you already have

```sh
buri add skills                            # here
buri add skills ~/src/some-other-repository
```

`buri add skills` writes the same skills into `.agent/skills/` without
touching anything else, so it works in a repository that predates them — and in
a directory that is not a Buri repository at all, since the skills are compiled
into the binary the way the rest of `buri docs` is.

Re-running is the upgrade path. A skill directory whose name begins `buri-`
belongs to the toolchain and is rewritten from the binary every run, and one
that a release has stopped shipping is removed. A directory named anything else
is yours and is never read, written, or removed.

## Next

[Your first program](./first-program.md) comes next.
