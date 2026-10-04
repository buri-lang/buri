---
title: A tool answers
message: '`{tool}` did not answer'
note: '{why}'
fix: fix what the note names; `buri test` on the tool runs its own tests
reproduction: none
---
# A tool answers

```text
= the tool was killed by SIGSEGV (signal 11)
  RangeError: Maximum call stack size exceeded.
```

The note says what went wrong: the tool didn't build, exited non-zero, was
killed by a signal, wrote nothing, or wrote something that isn't an answer.

A killed tool names its signal: `SIGSEGV` for a crash or a stack overflow,
`SIGKILL` for a machine out of memory. The last four kilobytes of its standard
error come with the note.

A slow tool isn't this error. The build waits for a tool as long as it runs,
the way it waits for a compiler.
