## What it does

Prints the toolchain version on one line. It works outside a repository.

`--verbose` adds the running executable's identity: the id its linker wrote
into it (`LC_UUID` on macOS, the GNU build id on Linux). Two builds of one
version are different compilers, and the identity tells them apart, so put it in
bug reports. It replaces the toolchain pin `REPO.buri` once carried
([`repo-config.md`](../build/repo-config.md#what-is-not-here)).
