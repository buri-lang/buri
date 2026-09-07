# The getting-started tutorial's repository

Every fence on `cli/src/docs/getting-started/tutorial.md` names this directory —
`repo=cli/tests/tutorial`, plus the `package=` the file belongs to — so the docs
suite compiles the page against a repository that really exists.

**The files here are byte-for-byte what the page shows.** A reader who types the
page out ends up with this repository. Edit one side without the other and you
have a bug even where it still compiles: the comments are part of what the page
teaches.

`buri test` runs six suites, `buri lint` reports nothing, `buri format` rewrites
nothing, and `buri run //apps/convert -- 26.2 mi km` prints `26.2 mi = 42.16 km`.
