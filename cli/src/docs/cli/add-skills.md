## What it does

Writes the agent skills this toolchain ships into `.claude/skills/`, one
directory per skill, each holding a `SKILL.md`. With no argument it writes into
the working directory; with one it writes into the directory you name. It needs
no repository, because the skills are compiled into the binary the way the rest
of `buri docs` is.

```text
buri add-skills
buri add-skills ~/src/some-other-repository
```

Five skills are installed today: the language, the type system, the build
system, testing, and this CLI. Each is the same prose `buri docs` serves,
compressed to what an agent meeting Buri for the first time needs in front of
it.

## Re-running is the upgrade

```text
wrote .claude/skills/buri-language/SKILL.md
overwrote .claude/skills/buri-types/SKILL.md
removed .claude/skills/buri-retired
```

A skill directory whose name begins `buri-` is **this toolchain's**. Every run
rewrites all of them from the binary, and removes any that a release has
stopped shipping — so upgrading the compiler and running this command again is
how the skills stop being out of date, and there is nothing to merge.

A directory named anything else is somebody's own. It is never read, never
written, and never removed, which is why the marker is a prefix on the name
rather than a manifest file: the directory is the only thing both sides can
see.
