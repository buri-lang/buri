---
title: A tool answers
message: '`{tool}` did not answer'
note: '{why}'
fix: fix what the note names; `buri test` on the tool runs its own tests
reproduction: none
---
# A tool answers

The note says which way it went: the tool did not build, it exited non-zero, a
signal killed it, it wrote nothing, or what it wrote is not an answer.

```text
= the tool was killed by SIGSEGV (signal 11)
  RangeError: Maximum call stack size exceeded.
```

A tool that was killed chose no status, so the note names the signal instead:
`SIGSEGV` for a crash or a stack it ran off the end of, `SIGKILL` for a machine
that ran out of memory. The last four kilobytes of what it put on standard error
come with the note, because what a program says last is what says why it
stopped.

A tool that is *slow* is not this page. Nothing puts a clock on a tool: the
build waits for as long as it runs, the way it waits for a compiler or a linker.
