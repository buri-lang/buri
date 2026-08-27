## What it does

Rewrites the seven fields that restate the sources of build files that already
exist — `sources`, `proto_sources`, `dependencies`, `test.sources`,
`test.dependencies`, `testing.sources` and `testing.dependencies` — from what the source tree
actually contains and what its modules actually import. Nothing else in the
file is touched: rules, tags, platforms, visibility, outputs, `test.data`, and
comments survive.

With no target argument it regenerates every package in the repository: bare
`buri gen` is `buri gen //...`. Every other command with no argument means the
package containing the working directory, and this one does not, because it
restates what the tree contains rather than answering a question about the code
in front of you — a tree restated one directory at a time is one where
`gen --check` passes where you are standing and fails one directory over. It is
the default `buri format` already has, and the two commands are meant to agree
about a file.

A managed list comes back **sorted**, because `gen` decides what is in it and
nothing about the order of a `sources` or `dependencies` entry means anything.
`buri format` sorts nothing — it leaves every list in the order it was written,
so a hand-written `tags` list is not rearranged behind you. The two commands
therefore never fight over a file: what `gen` writes is what `format --check`
accepts.

In a package with both a library and a binary, a file that no rule lists yet
goes to the rule whose entry point reaches it. A file reached from both, or
from neither, is an error naming the file: guessing there would move code
across a boundary that exists to be explicit.

It never creates a build file. A package exists because somebody decided it
should, and that decision is not one a tool should make by noticing a
directory.

The `--check` form writes nothing and exits `1` if anything would change.
