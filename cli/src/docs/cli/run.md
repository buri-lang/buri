## What it does

Builds exactly one binary target and executes it. Everything after `--` is
passed to the program rather than read as an argument to `buri`.

## Authority

This is the one command that produces a process with real authority: it runs
outside any sandbox, with the real filesystem and the real environment. What
the program can actually do is still bounded by the context its `main` builds —
a program whose `main` never names `host.net` cannot open a socket, because
nothing anywhere in it can obtain a value bounded by `Net`.
