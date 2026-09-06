# Installing

There is no release yet, so every path below builds from source. They all
produce the same binary, and differ only in what supplies the Rust toolchain.

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

`--HEAD` builds the `main` branch. You need it until a release is tagged, then
drop it.

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

That last row is why you run `init` rather than copy files out of a page. A
coding agent working in the repository gets the toolchain's own account of the
language, the type system, the build system, testing, and the CLI, written by
the release you installed. Each skill is also a page here, starting with
[the language skill](../reference/skills/buri-language.md).

`buri init` never writes over your work. A `REPO.buri` at the target means the
directory is already a repository, so the command stops. A `REPO.buri` *above*
the target stops it too: a repository inside another one is not a root, it is a
stray build file in somebody else's tree. A `.gitignore` already at the target
is the one exception, since running `git init` first is the ordinary way to
start. There `buri init` appends its entries below your lines instead of
refusing to run.

## Skills in a repository you already have

```sh
buri add skills                            # here
buri add skills ~/src/some-other-repository
```

`buri add skills` writes the same skills into `.agent/skills/` and touches
nothing else. It works in a repository that predates them, and in a directory
that is not a Buri repository at all, because the skills are compiled into the
binary the way the rest of `buri docs` is.

Run it again to upgrade. A skill directory whose name begins `buri-` belongs to
the toolchain, so every run rewrites it from the binary and removes any the
release has stopped shipping. A directory named anything else is yours, and
`buri add skills` never reads, writes, or removes it.

## Next

[Your first program](./first-program.md) comes next.
