## What it does

Formats `.buri` sources and `BUILD.buri` files in place. There are no options:
one canonical layout, so formatting is never something to argue about in
review and never something a repository has to configure.

Formatting is a fixed point — running it twice changes nothing the second time —
which is what lets `buri gen` and `buri format` write the same file without
fighting over it.

The `--check` form writes nothing and exits `1` if anything would change. That
is the form for a continuous-integration job.
