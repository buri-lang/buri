---
title: Every declaration is reached from a `lib.buri` or a `main.buri`
severity: warning
message: nothing reaches `{name}`
note: production code is reached from a library's surface or from a binary's `main`, and a declaration nothing reaches is a declaration nothing runs
fix: delete it, or re-export it from {library_file} to put it on the library's surface
---
Inside a library, `export` means "visible to this library", and `lib.buri`
decides what leaves it. An `export` that `lib.buri` doesn't re-export and no
sibling imports is reached by nothing. The two fixes are opposites, so decide
which applies:

- **It's meant to be published.** Add it to the `lib.buri` re-export of its
  module. The rule never reports a name on the surface.
- **It's dead.** Delete it, along with whatever existed only to support it.

A module under `testing/` is measured against `testing/lib.buri` instead.

Using it from a test is not a fix, because a test is not a use. Test it through
the function the library publishes. If the logic needs its own tests, make it
its own published module.

The rule only looks at module-level declarations. Fields and variants are
`unused-field`'s and `unused-variant`'s job, and test sources and `test`
declarations are never reported. It also stays quiet:

- in a binary, which has no surface;
- for a module imported with `import * as`, which reaches every name it exports;
- in a package with a file the parser couldn't read whole, since the missing
  part might hold the import.
