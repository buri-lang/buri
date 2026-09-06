## What it does

Builds exactly one binary target and runs it. Everything after `--` goes to the
program instead of to `buri`.

With no target argument it matches the whole repository, so bare `buri run`
works in a repository that declares one binary and means that one. Where it
matches several, the error names them and you pick.

## Authority

This is the one command that starts a process with real authority. It runs
outside the build graph, with the real filesystem and the real environment.

The context its `main` builds still bounds what the program can do. A program
whose `main` never names `host.net` cannot open a socket, because nothing
anywhere in it can obtain a value bounded by `Net`.
