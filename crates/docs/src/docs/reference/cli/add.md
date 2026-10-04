## What it does

`buri add` writes into an existing repository, one subcommand per thing a
release can add. `buri init` creates a repository once and never touches it
again. With no subcommand, `buri add` lists the subcommands and exits 2.

## `buri add skills`

Writes the agent skills this toolchain ships into `.agent/skills/`, one
directory per skill, each holding a `SKILL.md`. It writes into the working
directory or the one you name, and needs no repository.

```text
buri add skills
buri add skills ~/src/some-other-repository
```

It installs five skills: the language, the type system, the build system,
testing, and this CLI. Each is the prose `buri docs` serves, compressed to what
an agent new to Buri needs.

### Re-running is the upgrade

```text
wrote .agent/skills/buri-language/SKILL.md
overwrote .agent/skills/buri-types/SKILL.md
removed .agent/skills/buri-retired
```

Directories named `buri-*` belong to the toolchain. Every run rewrites them all
and removes any a release stopped shipping, so after upgrading the compiler, run
this again. Any other directory is never read, written or removed.
