## What it does

Compiles the targets you name. A binary produces an artifact under
`.buri/out/<platform>/<package>/`; a library is type-checked, because a library
has no artifact of its own — `buri build //lib/money` means "tell me whether
this library is correct."

With no target argument it builds the whole repository: bare `buri build` is
`buri build //...`, from any directory in it.

## Caching

A build is a set of actions, each keyed on the toolchain version, the build
mode, the platform, and the content of every input. An action whose key is
already in the cache is served from it rather than run, so a second build of an
unchanged tree does no work. The key is content-addressed, so moving the
checkout, or building the same commit on another machine, hits the same
entries.

Cache writes are serialized by a file lock and reads take none, so any number of
`buri` processes can work in one repository at once.

## Reproducibility

Two builds of one commit in one configuration produce byte-identical artifacts.
`--check-reproducible` asks that of this repository and exits 1 naming the
first byte that moved if it does not hold. It is not part of an ordinary build,
and what makes it a check rather than a ritual is set out in
[`hermeticity.md`](../build/hermeticity.md#reproducibility).
