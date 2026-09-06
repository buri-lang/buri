## What it does

`buri add` writes into a repository that already exists. `buri init` creates
one, once, and refuses to touch it again. Everything a release adds to a
checkout that is already there arrives under `add` instead, one subcommand each.
There is one today.

With no subcommand it prints what you can ask it for and exits 2, the way bare
`buri` prints the command table: an incomplete invocation is the thing you asked
*with* being wrong.

## `buri add skills`

Writes the agent skills this toolchain ships into `.agent/skills/`, one
directory per skill, each holding a `SKILL.md`. With no argument it writes into
the working directory; with one it writes into the directory you name. It needs
no repository, because the skills are compiled into the binary the way the rest
of `buri docs` is.

```text
buri add skills
buri add skills ~/src/some-other-repository
```

It installs five skills today: the language, the type system, the build system,
testing, and this CLI. Each one is the prose `buri docs` serves, compressed to
what an agent meeting Buri for the first time needs in front of it.

### Re-running is the upgrade

```text
wrote .agent/skills/buri-language/SKILL.md
overwrote .agent/skills/buri-types/SKILL.md
removed .agent/skills/buri-retired
```

A skill directory whose name begins `buri-` belongs to **this toolchain**.
Every run rewrites all of them from the binary and removes any that a release
has stopped shipping. So upgrade the compiler, run this command again, and the
skills are current. There is nothing to merge.

A directory named anything else is somebody's own. The command never reads it,
never writes it, and never removes it. That is why the marker is a prefix on the
name rather than a manifest file: the directory is the only thing both sides can
see.
