## What it does

Prints the toolchain version, then checks it against the version `REPO.buri`
pins. A mismatch is an error: the pin is an exact version and never a range,
because two checkouts of the same commit must not build with two different
compilers.

It also says whether the repository pins a `sha256` at all. A hash of nothing
but zeros is the sentinel for *unpinned*, which is the state a repository is in
while its toolchain is built from source; any other value is checked against the
hash of this executable, and a mismatch is the same error.

Outside a repository there is nothing to pin against, and that is not an error.

With `--verbose` it prints this executable's own hash, which is the value a
`REPO.buri` would write to pin it.
