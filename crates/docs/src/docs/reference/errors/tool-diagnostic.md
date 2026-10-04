---
title: A tool reports under a code the catalogue has
message: '{message}'
note: 'the tool reported this as `{code}`, which is not a code this toolchain has a page for'
fix: report it under a code the catalogue declares — `buri docs error <code>` is what a reader runs next
reproduction: none
---
# A tool reports under a code the catalogue has

The message above is the tool's own; only the code is wrong. A tool can't invent
a catalogue entry, but it can reuse any code the catalogue has, and its message
then prints under that code.

A diagnostic with an origin lands at that span of that file. One without lands
at the start of the file a `check` was handed, or on the `generators` entry that
ran a `generate`.
