## What it does

Prints the toolchain version. One line, and it is answered from the binary, so
it works outside a repository — there is nothing in a repository this command
needs.

With `--verbose` it also prints the SHA-256 of the running executable. Two
builds of one version are two compilers, and this is the only way to say which
one is here, which is what a bug report has to name.

`REPO.buri` used to pin a toolchain by version and hash, and this command used
to report that pin. The pin was removed; see
[`repo-config.md`](../build/repo-config.md#what-is-not-here).
