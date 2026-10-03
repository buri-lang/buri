## What it does

Rewrites the six fields of an existing build file that restate its sources
(`sources`, `dependencies`, `test.sources`, `test.dependencies`,
`testing.sources` and `testing.dependencies`) from what the tree holds and what
its modules import. Everything else survives: rules, generators, tags,
platforms, visibility, outputs and comments.

`generators` is left alone on purpose: nothing can tell which generator owns a
new file, so an entry and its `inputs` are yours.

Bare `buri gen` means `buri gen //...`, like `buri format`. Regenerating one
directory at a time would let `gen --check` pass where you are and fail next
door.

Managed lists come back sorted. `buri format` sorts no lists, so it never
reorders a hand-written `tags` list, and what `gen` writes passes
`format --check`.

In a package with both a library and a binary, a file no rule lists yet goes to
the rule whose entry point reaches it. A file reached from both, or neither, is
an error naming the file.

It never creates a build file.

`--check` writes nothing and exits `1` if anything would change.
