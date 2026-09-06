## What it does

Formats `.buri` sources and build files — `BUILD.buri` and `REPO.buri` — in
place, **and the Buri written in documentation**: every ```` ```buri ```` fence
in a markdown file, and every example in a `///` or `//!` comment. There are no
options. One canonical layout means nobody argues about formatting in review and
no repository has to configure it.

Format the sources but not the examples and you have two house styles, and the
one a newcomer copies is the one in the prose. So one command lays out all of
it, and `--check` gates all of it. In a document only fence bodies change; the
prose around them is yours.

Formatting is a fixed point: run it twice and the second run changes nothing.
That is what lets `buri gen` and `buri format` write the same file without
fighting over it.

The formatter **sorts** the leading run of imports: `core/*` before `//*`, then
by path, then by clause, with one blank line between the two groups and none
inside either. A module's imports are a set, so their order carries no meaning.
Leave that order to the author and every diff that adds an import becomes a
choice somebody has to make and somebody else has to review. This is why there
is no `unsorted-imports` lint: an unsorted run is a file nobody formatted, not a
finding to report.

Only the *leading* run moves. An import written after a declaration stays where
it is, because moving it across that declaration could change what the module
means.

## Type declarations

Width decides almost every other break — what is left of the line against what
comes next. Struct and enum declarations ignore it. A **struct** declaration
with a braced field list puts every field on its own line, and an **enum**
declaration puts every variant on its own line, each with a trailing comma,
however short the whole would be:

```buri
export enum Hello {
    World,
    Now(Bool),
}
```

One field or one variant breaks exactly like ten. You scan these declarations to
learn what a program is made of, and a type should not read one way at three
fields and another way at four. The trailing comma also makes adding a member a
one-line diff.

Two shapes have nothing to break. An empty body stays shut — `struct S {}` and
`enum E {}` — because there is no member to give a line to and no list to put a
comma on. A tuple struct has no braced field list at all, so
`struct Meters(F64);` stays as you wrote it.

All of this is about *declarations*. A struct literal in an expression, and a
match arm, break on the width like everything else.

## A comment beside the code

A comment at the end of a line is about the code on that line, so it stays
there: one space after the code, whatever column you typed it in. Every other
comment goes on a line of its own, above the thing you wrote it above.

The comment never changes the layout of the code. The formatter measures a line
as if the comment were not there, so an aside can never break the call it
follows. It does not rewrap a comment either. Both halves say the same thing:
the line is as long as your sentence makes it.

## A file with a syntax error

A file you are editing has a syntax error in it most of the time, and it is
still worth laying out. So the declaration the parser could not read comes back
**exactly as you wrote it**, byte for byte, and the formatter lays out
everything around it as usual. It does not touch what it did not understand. The
whole declaration is the unit, because a recovered tree says where a mistake was
and not what you meant by the text around it.

Formatting such a file is still a fixed point. It still keeps every comment and
every token, and still fits the margin everywhere it laid something out. Inside
the region, the line lengths are yours.

`buri format` names each file it could only partly read, and `--check` **exits
`1`** for it, whether or not anything outside the region would change. A file
the formatter could not read whole is a file it has not checked, and a green
gate that got there by skipping a file is worse than a red one. So `--check`
fails on three things: a file that would change, a file with a syntax error, and
a file the formatter refused outright.

## Build files

A build file is data, so the same command decides its canonical form the same
way.

Fields come back in **the order the schema declares them** — `library` before
`binary`, `sources` before `dependencies` before `test`. Field order in a build
file carries no meaning, and the one order nobody has to argue about is the one
somebody wrote the schema in. A field the schema does not know keeps its place
at the end: the formatter neither moves it nor drops it, because rearranging
something it did not recognise would be worse than leaving it alone. Repeated
fields keep the order you wrote them in — two `tag` blocks, the entries of an
`outputs` list — because that order is the only thing about them that could mean
something.

The rest is layout: one field per line, four-space indent, `name: value` for a
scalar and `name { … }` for a block, a list on one line while it is short and
one element to a line with a trailing comma when it is not, and every comment
kept with the field beneath it.

`buri gen` writes build files through this same printer, so the two cannot fight
over a file: what `gen` leaves behind is what `format --check` accepts.

The `--check` form writes nothing and exits `1` if anything would change, or if
any source has a syntax error, as above. That is the form for a
continuous-integration job. A build file that does not read is a different
matter: nothing in the repository works until you fix it, so the run stops there
and exits `2`.
