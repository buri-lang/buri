---
title: Every declaration is reached from a `lib.buri` or a `main.buri`
severity: warning
message: nothing reaches `{name}`
note: production code is reached from a library's surface or from a binary's `main`, and a declaration nothing reaches is a declaration nothing runs
fix: delete it, or re-export it from {library_file} to put it on the library's surface
---
Inside a library, `export` means "visible to the rest of this library", and
`lib.buri` decides what leaves it. So an `export` that `lib.buri` does not
re-export, and that no sibling module imports, is reached by nothing at all.

There are two fixes and they are opposites, so decide which one this is before
you edit.

**It is meant to be published.** Name it in the `lib.buri` re-export that
already carries its module's other names. The lint never reports a name on the
surface, because the readers it can see are not the only readers there are.

**It is dead.** Delete it, along with whatever existed only to support it.
Version control remembers it.

A module under `testing/` answers the same question against its own surface,
`testing/lib.buri`.

Reaching for it from a test is not a fix. Nothing can import a test source, so a
test is not a use. Drive the behaviour through the function the library actually
publishes; if the logic wants a test of its own, that is the signal it should be
its own module, published on the surface in its own right.

The rule asks only about module-level declarations. A field's or a variant's
`export` is `unused-field`'s and `unused-variant`'s question. It says nothing
about a test source, which exports nothing, or about a `test` declaration.

Three more things quiet it, each because the evidence is missing. A binary has
no surface. A module taken whole by `import * as` reaches every name it exports.
And the rule skips a package whose modules the parser could not all read whole,
because the import that reaches this name may sit in the run of declarations the
parser skipped.
