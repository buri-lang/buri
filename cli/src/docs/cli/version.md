## What it does

Prints the toolchain version, then checks it against the version `REPO.buri`
pins. A mismatch is an error: the pin is an exact version and never a range,
because two checkouts of the same commit must not build with two different
compilers.

Outside a repository there is nothing to pin against, and that is not an error.
