## What it does

Formats `.buri` sources and build files — `BUILD.buri` and `REPO.buri` — in
place. There are no options: one canonical layout, so formatting is never
something to argue about in review and never something a repository has to
configure.

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

## A file with a syntax error

A file being edited is a file with a syntax error in it for most of the time
you are editing it, and it is still worth laying out. So the declaration the
parser could not read comes back **exactly as it was written**, byte for byte,
and everything around it is formatted as usual. What the formatter did not
understand, it does not touch: the whole declaration is the unit, because a
recovered tree says where a mistake was and not what you meant by the text
around it.

Formatting such a file is still a fixed point, still keeps every comment and
every token, and still fits the margin everywhere it laid something out. Inside
the region, the line lengths are yours.

`buri format` names each file it could only partly read, and `--check` **exits
`1`** for it — whether or not anything outside the region would change. A file
the formatter could not read whole is not a file it has checked, and a green
gate that got there by skipping a file is worse than a red one. So `--check`
fails on three things: a file that would change, a file with a syntax error,
and a file the formatter refused outright.

## Build files

A build file is data, so its canonical form is decided the same way and by the
same command.

Fields come back in **the order the schema declares them** — `library` before
`binary`, `sources` before `dependencies` before `test`. The order of a field
in a build file carries no meaning, and the one order nobody has to argue about
is the one the schema was written in. A field the schema does not know keeps
its place at the end rather than being moved or dropped: a formatter that
rearranged something it did not recognise would be worse than one that left it
alone. Repeated fields — two `tag` blocks, the entries of an `outputs` list —
keep the order they were written in, because that order is the only thing about
them that could mean something.

The rest is layout: one field per line, four-space indent, `name: value` for a
scalar and `name { … }` for a block, a list on one line while it is short and
one element to a line with a trailing comma when it is not, and every comment
kept with the field beneath it.

`buri gen` writes build files through this same printer, so the two cannot
fight over a file: what `gen` leaves behind is what `format --check` accepts.

The `--check` form writes nothing and exits `1` if anything would change — or
if any source has a syntax error, as above. That is the form for a
continuous-integration job. A build file that does not read is a different
matter: nothing in the repository works until it is fixed, so the run stops
there and exits `2`.
