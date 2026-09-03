---
title: Every declaration is reached from a `lib.buri` or a `main.buri`
severity: warning
message: nothing reaches `{name}`
note: production code is reached from a library's surface or from a binary's `main`, and a declaration nothing reaches is a declaration nothing runs
fix: delete it, or re-export it from {library_file} to put it on the library's surface
---
Inside a library, `export` means "visible to the rest of this library", and
`lib.buri` decides what leaves it. So an `export` that `lib.buri` does not
re-export, and that no sibling module in the package imports, is reached by
nothing at all: not by a consumer, which cannot see it, and not by the library
itself, which never names it. The word in front of the declaration says
otherwise, which is why this is worth a sentence rather than a shrug.

There are two fixes and they are opposites, so decide which one this is before
you edit.

**It is meant to be published.** Name it in the `lib.buri` re-export that
already carries its module's other names, and the finding ends. A name on the
surface is never reported, because the reader this analysis can see is not the
only reader there is.

**It is dead.** Delete it, along with whatever existed only to support it.
Version control remembers it, and a declaration kept "just in case" costs every
later reader the work of deciding whether it matters.

What is not a fix is reaching for it from a test. A test source cannot be
imported and is not a use; an export that exists so a test can see an internal
couples the test to the implementation and makes the library's surface lie.
Drive the behaviour through the function the library actually publishes. If the
logic is substantial enough to want a test of its own, that is the signal it
should be its own module, published on the surface in its own right.

The rule asks only about module-level declarations. A field's or a variant's
`export` is about the shape of a type, and whether anything reads it is
`unused-field`'s and `unused-variant`'s question. Three things also quiet it,
each because the evidence is not there: a binary has no surface, so nothing in
one is asked; a module taken whole by `import * as` reaches every name it
exports, so nothing in that module is asked; and a package one of whose modules
the parser could not read whole is not asked at all, because the import that
reaches this name may be in the run of declarations the parser skipped.
