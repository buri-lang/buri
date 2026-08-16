## What it does

Compiles the targets you name. A binary produces an artifact under
`.buri/out/<platform>/<package>/`; a library is type-checked, because a library
has no artifact of its own — `buri build //lib/money` means "tell me whether
this library is correct."

With no target argument it builds the package containing the working directory.

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
`--check-reproducible` asks that of this repository: it builds each requested
binary twice, from two freshly opened sessions, with the cache off, into two
separate directories, and compares the bytes. Silent on agreement; on a difference
it names the artifact and the first byte that moved, and exits 1. It writes no
artifact of its own.
