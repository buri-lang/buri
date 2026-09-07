## What it does

Builds exactly one binary target and runs it. Everything after `--` goes to the
program instead of to `buri`.

With no target argument it matches the whole repository, so bare `buri run`
works in a repository that declares one binary. Where it matches several, the
error names them and you pick.

A binary with several outputs runs the host's own platform where this toolchain
can build for it, and a page or a script otherwise. It never runs a
`CLOUDFLARE_WORKER` output: a worker is called by its platform, once per
request, so there is nothing to start. A binary that declares a page and a
worker runs the page, and one that declares a worker and nothing else is
refused:

```text
$ buri run //cmd/worker
error: //cmd/worker declares no output this toolchain can run
  = declared: cloudflare-worker
  = a worker is called by its platform, once per request, so there is nothing to start
  = fix: build it with `buri build //cmd/worker`, and let the platform call it
```

## Authority

This is the one command that starts a process with real authority. It runs
outside the build graph, with the real filesystem and the real environment.

The context its entry builds still bounds what the program can do. A program
whose entry never names `host.net` cannot open a socket, because nothing
anywhere in it can obtain a value bounded by `Net`.
