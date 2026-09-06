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
nothing at all. Not by a consumer, which cannot see it. Not by the library
itself, which never names it. The word in front of the declaration says
otherwise, so this is worth a sentence rather than a shrug.

There are two fixes and they are opposites, so decide which one this is before
you edit.

**It is meant to be published.** Name it in the `lib.buri` re-export that
already carries its module's other names, and the finding ends. The lint never
reports a name on the surface, because the readers it can see are not the only
readers there are.

**It is dead.** Delete it, along with whatever existed only to support it.
Version control remembers it, and a declaration kept "just in case" costs every
later reader the work of deciding whether it matters.

A module under `testing/` answers the same question against its own surface,
`testing/lib.buri`. A fake that file does not carry, and that no module beside
it imports, is reached by nobody. A suite that imports the testing surface names
what it takes, which is what keeps a live fixture quiet.

Reaching for it from a test is not a fix. Nothing can import a test source, so a
test is not a use. An export that exists so a test can see an internal couples
the test to the implementation and makes the library's surface lie. Drive the
behaviour through the function the library actually publishes. If the logic is
substantial enough to want a test of its own, that is the signal it should be
its own module, published on the surface in its own right.

The rule asks only about module-level declarations. A field's or a variant's
`export` is about the shape of a type, and `unused-field` and `unused-variant`
ask whether anything reads it. The rule says nothing about a test source, which
exports nothing, or about a `test` declaration, which the runner reaches rather
than the program.

Three more things quiet it, each because the evidence is missing. A binary has
no surface, so the rule asks nothing about one. A module taken whole by
`import * as` reaches every name it exports, so the rule asks nothing about that
module. And the rule skips a package whose modules the parser could not all read
whole, because the import that reaches this name may sit in the run of
declarations the parser skipped.
