## What it does

Rewrites the `sources` and `dependencies` fields of build files that already
exist, from what the source tree actually contains and what its modules
actually import. Nothing else in the file is touched: rules, tags, visibility,
and comments survive.

It never creates a build file. A package exists because somebody decided it
should, and that decision is not one a tool should make by noticing a
directory.

The `--check` form writes nothing and exits `1` if anything would change.
