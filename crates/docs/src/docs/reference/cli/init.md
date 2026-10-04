## What it does

Writes a working repository into an empty directory: a `REPO.buri` root, a
library, a binary that depends on it, a test suite, a `.gitignore`, and the agent
skills `buri add skills` installs. It writes into the working directory, or into
the directory you name, creating it if needed.

```text
buri init
buri init hello-buri
```

The result builds, tests, lints and formats clean from the start: a first
`buri format` changes nothing and a first `buri gen --check` reports nothing.

```text
wrote REPO.buri
wrote .gitignore
wrote libs/greeting/BUILD.buri
wrote libs/greeting/lib.buri
wrote libs/greeting/greeting.buri
wrote libs/greeting/test/greeting.buri
wrote apps/hello/BUILD.buri
wrote apps/hello/main.buri
wrote .agent/skills/buri-language/SKILL.md
```

## What it generates

`//libs/greeting` is a library in two files, the smallest a library can be.
`lib.buri` is its public surface and may hold only re-exports, so it needs a
module behind it:

```buri
/// The greeting this repository was born with.
export fn greeting(): Str {
    "hello world"
}
```

`//apps/hello` is the binary. Its `main` builds a context holding allocation and
standard output, and that's the program's whole effect budget: nothing it calls
can read a file or open a socket.

The suite under `libs/greeting/test/` imports the library by label, like any
dependent, so it can only test what a dependent can call. Run it with
`buri test //...`.

`REPO.buri` declares no tags, just a comment pointing at
[`schema/repo.proto`](../schema/repo.proto), which lists every field it may
declare. It does turn on both fields of the `lint` block, so `buri build` and
`buri test` run the lint catalogue and fail on a finding. Neither is the default,
but a fresh repository has no findings to clean up first. Delete the block to
opt out ([`repo-config.md`](../build/repo-config.md#lint)).

## It never writes over your work

A `REPO.buri` at the target stops the command with exit 2. A scaffold is a
starting point, not something to refresh; re-running `buri add skills` is how
you upgrade skills.

A `REPO.buri` *above* the target stops it too. The repository root is the
outermost `REPO.buri`, so an inner one would be a stray build file that breaks
the outer repository's `buri build //...`.

Any other existing file stops it before it writes anything, so a refusal never
leaves half a repository behind.

```text
error: `./apps/hello/main.buri` already exists; `buri init` never writes over a file
```

## Except your `.gitignore`

You'll often run `git init` first, so an existing `.gitignore` is merged into
rather than refused:

```text
wrote REPO.buri
updated .gitignore
```

Entries match whole lines, trailing spaces included. Your lines and comments stay
where they are, and only the missing entries are appended. A file that already
ignores everything the build writes is left untouched:

```text
kept .gitignore
```

The only other shared namespace is `.agent/skills/buri-*`, which follows
`add skills`'s rules.
