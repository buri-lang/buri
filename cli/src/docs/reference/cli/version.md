## What it does

Prints the toolchain version, one line, straight from the binary. It needs
nothing from a repository, so it works outside one.

`--verbose` adds the SHA-256 of the running executable. Two builds of one
version are two different compilers, and the hash is the only way to say which
one you have. That is what a bug report has to name. It replaces the toolchain
pin `REPO.buri` once carried
([`repo-config.md`](../build/repo-config.md#what-is-not-here)).
