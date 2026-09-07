---
title: A generator answers with a response
message: the generator produced no response
fix: run the tool by hand and check it writes one line of JSON on standard output
reproduction: none
---
# A generator answers with a response

The note says which way it went: the tool did not build, it exited non-zero, it
wrote nothing, what it wrote is not a response, or it ran past the build's
deadline. Whatever the tool put on standard error comes with the note.

A response is one line of JSON — the modules to load, and what the generator has
to say about its inputs:

```text
{"modules":[{"name":"units","text":"export let width: Int = 3;\n","anchors":[]}],"diagnostics":[]}
```

The response is the last non-empty line of standard output, so a `println` of
your own above it costs nothing. `core/codegen`'s `run` writes the line for you,
and a tool built on it reaches this page only when it never got as far as
answering.
