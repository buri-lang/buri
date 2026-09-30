---
title: A tool reports under a code the catalogue has
message: '{message}'
note: 'the tool reported this as `{code}`, which is not a code this toolchain has a page for'
fix: report it under a code the catalogue declares — `buri docs error <code>` is what a reader runs next
reproduction: none
---
# A tool reports under a code the catalogue has

The sentence above is the tool's own. Only the code is this page's: a tool
cannot invent a catalogue entry, and a code with no page has no wording anybody
can hold it to.

A code the catalogue does have prints under that code instead, with the tool's
own sentence. So a tool can reuse the pages this toolchain already writes.

A diagnostic with an origin lands at that span of that file. One without lands
on the start of the file a `check` was handed, or on the `generators` entry that
ran a `generate`.
