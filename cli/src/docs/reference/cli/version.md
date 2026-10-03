## What it does

Prints the toolchain version on one line. It works outside a repository.

`--verbose` adds the SHA-256 of the running executable. Two builds of one
version are different compilers, and the hash tells them apart, so put it in bug
reports. It replaces the toolchain pin `REPO.buri` once carried
([`repo-config.md`](../build/repo-config.md#what-is-not-here)).
