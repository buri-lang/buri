## What it does

Writes a working repository into an empty directory: a `REPO.buri` root, one
library, one binary that depends on it, one test suite, a `.gitignore`, and the
agent skills `buri add-skills` installs. With no argument it writes into the
working directory; with one it writes into the directory you name, creating it
if it is not there.

```text
buri init
buri init hello-buri
```

The result builds, tests, lints and formats clean the moment it lands — the
generated sources *are* the formatter's own output, so a first `buri format`
changes nothing and a first `buri gen --check` reports nothing.

```text
wrote REPO.buri
wrote .gitignore
wrote libs/greeting/BUILD.buri
wrote libs/greeting/lib.buri
wrote libs/greeting/greeting.buri
wrote libs/greeting/test/greeting.buri
wrote apps/hello/BUILD.buri
wrote apps/hello/main.buri
wrote .claude/skills/buri-language/SKILL.md
```

## What it generates

`//libs/greeting` is a library in two files, which is the smallest a library can
be: `lib.buri` is its public surface and may list nothing but re-exports, so it
needs at least one module behind it to re-export from.

```buri
/// The greeting this repository was born with.
export fn greeting(): Str {
    "hello world"
}
```

`//apps/hello` is the binary. Its `main` builds a context holding two effects —
allocation and standard output — and that context is the program's entire
effect budget: nothing it calls can read a file or open a socket, because
nothing handed it the means to.

The suite under `libs/greeting/test/` imports the library by label, exactly as a
dependent does, so it can only assert on what a dependent can call. Run it with
`buri test //...`.

The `REPO.buri` declares no tags. A repository with no build policy has nothing
to say there, and what stands in their place is a comment telling the next
reader where a `tag` block goes.

What it does declare is a `lint` block with both of its fields on, so `buri
build` and `buri test` run the lint catalogue from the first commit and a
finding fails them. Neither is the default, and a fresh repository is exactly
where the strictest setting is free: there is no accumulated finding to clean up
before adopting it, and every one raised from here on is raised on code somebody
is still writing. Deleting the block is one edit; discovering that it could have
been there is a year of findings nobody was shown
([`repo-config.md`](../build/repo-config.md#lint)).

## It never writes over your work

A `REPO.buri` at the target means the directory is already a repository, and
the command stops with exit 2 rather than refreshing it — there is no upgrade
path here, because a scaffold is a starting point and not something a release
keeps in step. That is the difference from `buri add-skills`, where re-running
*is* the upgrade.

A `REPO.buri` *above* the target stops it as well, and for a sharper reason.
Nesting one writes over nothing, but the toolchain finds a repository root by
walking up to the outermost `REPO.buri` it meets — so the inner one is not a
root, it is a stray build file inside somebody else's repository, and their
next `buri build //...` fails on it. `buri init` in a subdirectory of a
repository therefore says so and stops.

Any other collision stops it too, and stops it before the first byte is
written, so a refusal never leaves half a repository behind. The one namespace
the command shares is `.claude/skills/buri-*`, which belongs to `add-skills`
and follows its rules.
