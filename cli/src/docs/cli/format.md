## What it does

Formats `.buri` sources and `BUILD.buri` files in place. There are no options:
one canonical layout, so formatting is never something to argue about in
review and never something a repository has to configure.

Formatting is a fixed point — running it twice changes nothing the second time —
which is what lets `buri gen` and `buri format` write the same file without
fighting over it.

The leading run of imports is **sorted**: `core/*` before `//*`, then by path,
then by clause, with one blank line between the two groups and none inside
either. A module's imports are a set, so their order carries no meaning — and
leaving it to the author makes every diff that adds one a choice somebody has
to make and somebody else has to review. This is why there is no
`unsorted-imports` lint: an unsorted run is a file that has not been formatted,
not a finding to report.

Only the *leading* run moves. An import written after a declaration stays where
it is, because moving it across that declaration could change what the module
means.

The `--check` form writes nothing and exits `1` if anything would change. That
is the form for a continuous-integration job.
