## What it does

Rewrites the six fields of an existing build file that restate its sources:
`sources`, `dependencies`, `test.sources`, `test.dependencies`,
`testing.sources` and `testing.dependencies`. It reads what the source tree
actually holds and what its modules actually import. It touches nothing else, so
rules, generators, tags, platforms, visibility, outputs, and comments all
survive.

`generators` is the field it deliberately leaves alone: nothing can work out
which generator owns a new file, so an entry and its `inputs` are yours.

With no target argument it regenerates every package in the repository: bare
`buri gen` is `buri gen //...`. That default matters most here. Restate a tree
one directory at a time and `gen --check` passes where you are standing but
fails one directory over. `buri format` has the same default, and the two
commands are meant to agree about a file.

A managed list comes back **sorted**, since the order of a `sources` or
`dependencies` entry means nothing. `buri format` sorts nothing: it leaves every
list in the order you wrote it, so it never rearranges a hand-written `tags`
list behind you. What `gen` writes is what `format --check` accepts.

In a package with both a library and a binary, a file that no rule lists yet
goes to the rule whose entry point reaches it. A file reached from both, or from
neither, is an error naming the file.

It never creates a build file. A package exists because somebody decided it
should.

The `--check` form writes nothing and exits `1` if anything would change.
