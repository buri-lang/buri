## What it does

Rewrites the seven fields of an existing build file that restate its sources:
`sources`, `proto_sources`, `dependencies`, `test.sources`,
`test.dependencies`, `testing.sources` and `testing.dependencies`. It reads
what the source tree actually holds and what its modules actually import. It
touches nothing else, so rules, tags, platforms, visibility, outputs, and
comments all survive.

With no target argument it regenerates every package in the repository: bare
`buri gen` is `buri gen //...`, the same default every other command has. That
default matters most here. Restate a tree one directory at a time and
`gen --check` passes where you are standing but fails one directory over.
`buri format` has the same default, and the two commands are meant to agree
about a file.

A managed list comes back **sorted**. `gen` decides what goes in it, and the
order of a `sources` or `dependencies` entry means nothing. `buri format` sorts
nothing: it leaves every list in the order you wrote it, so it never rearranges
a hand-written `tags` list behind you. The two commands therefore never fight
over a file, and what `gen` writes is what `format --check` accepts.

In a package with both a library and a binary, a file that no rule lists yet
goes to the rule whose entry point reaches it. A file reached from both, or from
neither, is an error naming the file. Guessing there would move code across a
boundary that exists to be explicit.

It never creates a build file. A package exists because somebody decided it
should, and no tool should make that decision by noticing a directory.

The `--check` form writes nothing and exits `1` if anything would change.
