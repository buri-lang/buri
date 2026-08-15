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
