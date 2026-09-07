---
title: A generator answers with a response
message: the generator produced no response
fix: run the tool by hand and check it writes one line of JSON on standard output
reproduction: none
---
# A generator answers with a response

The note says which way it went: the tool did not build, it exited non-zero, a
signal killed it, it wrote nothing, what it wrote is not a response, or it ran
past the build's deadline.

```text
= the generator was killed by SIGSEGV (signal 11)
  RangeError: Maximum call stack size exceeded.
```

A tool that was killed chose no status, so the note names the signal instead —
`SIGSEGV` for a crash or a stack it ran off the end of, `SIGKILL` for a machine
that ran out of memory. Whatever it put on standard error comes with the note,
the last four kilobytes of it, because what a program says last is what says
why it stopped.

A response is one line of JSON — the modules to load, and what the generator has
to say about its inputs:

```text
{"modules":[{"name":"units","text":"export let width: Int = 3;\n","anchors":[]}],"diagnostics":[]}
```

The response is the last non-empty line of standard output, so a `println` of
your own above it costs nothing. `core/codegen`'s `run` writes the line for you,
and a tool built on it reaches this page only when it never got as far as
answering.
