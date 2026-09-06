# The getting-started tutorial's repository

`cli/src/docs/getting-started/tutorial.md` walks a reader through building this
repository one file at a time. Every fence on that page names this directory —
`repo=cli/tests/tutorial`, plus the `package=` the file belongs to — so the docs
suite compiles the page against a repository that really exists. A page that
drifts from the language stops compiling.

**The files here are byte-for-byte what the page shows.** A reader who types the
page out ends up with this repository, and `buri test` here is the run the page's
transcripts came from. Edit one side without the other and you have a bug, even
where it still compiles: the comments are part of what the page teaches.

`buri test` runs six suites, `buri lint` reports nothing, `buri format` rewrites
nothing, and `buri run //apps/convert -- 26.2 mi km` prints `26.2 mi = 42.16 km`.
