## What it does

Builds exactly one binary target and runs it. Everything after `--` goes to the
program instead of to `buri`.

With no target argument it matches the whole repository, so bare `buri run`
works in a repository that declares one binary. Where it matches several, the
error names them and you pick.

Stopping `buri run` stops the program it started: `SIGINT`, `SIGTERM` and
`SIGHUP` reach the program, `buri run` waits for it to finish shutting down, and
exits with the program's own status — 128 plus the signal where one ended it.

A binary with several outputs runs the host's own platform where this toolchain
can build for it, and a page or a script otherwise.

A [repository platform](../build/platforms.md)'s entry runs when it starts
itself: no `js` file, and the program signature, `fn(host: H): Result<(), Str>`.
An entry with a `js` file, like a worker's `fetch`, is called by whatever loads
that file. A binary that declares a page and a worker serves the page, and one
that declares a worker and nothing else is refused:

```text
$ buri run //cmd/site
error: //cmd/site builds `fetch` for `//platform/cloudflare_worker`, and its `js` file calls it, so there is nothing to start [entry-not-runnable]
```

## A page is served

An output whose platform ships an `index.html`, like `web`, is a document a
browser loads, so there is no process to start. `buri run` builds it and serves
its output directory on a local port instead. A
[repository platform](../build/platforms.md) gets the same by listing
`index.html` in its `assets`:

```text
$ buri run //apps/design
serving //apps/design on http://127.0.0.1:4000/
```

The address is printed once, before anything blocks, and everything it answers
carries `Cache-Control: no-store` — a rebuild is what the next reload shows.

**Every path that is not a file is `index.html`.** A page routes on
`web.route(ctx)`, which is the address bar, so `/components/button` has to
arrive with `/components/button` still in it. Redirecting to `/index.html` would
hand the router `/index.html` and render the not-found route; answering 404
would break every deep link. A path whose last segment carries an extension is a file
instead: `/main.css` is the stylesheet, and `/theme.css`, which the build never
wrote, is a 404 rather than HTML claiming to be a stylesheet.

`--port` names another port, and `--port=0` takes whatever the operating system
has free and prints the number it got. A port already in use is refused rather
than quietly replaced.

`SIGINT`, `SIGTERM` or `SIGHUP` stops the server, which exits with 128 plus the
signal. Under `--watch` a rebuild in progress finishes first; a second signal
stops it at once.

`--watch` rebuilds on a change to a declared input — the same loop, the same
declared set and the same 150 ms sweep [`buri test`](./test.md#watching)
describes. The build rewrites the artifact directory in place, so the next
request answers from it; a rebuild that fails prints its diagnostics and leaves
the page that was working where it was. Both flags belong to a page: on a binary
that runs as a process they are refused, because there is no port and nothing to
rebuild into.

What it serves is the `index.html` the platform ships, not a document a worker
renders. **Running the worker in front of the page locally is not something
this command does**, so a page that calls `web.resume` finds markup no `shell`
wrote and says so. Mount that page while you work on it,
or put it behind the real worker.

## Authority

This is the one command that starts a process with real authority. It runs
outside the build graph, with the real filesystem and the real environment.

The context its entry builds still bounds what the program can do. A program
whose entry never names `host.net` cannot open a socket, because nothing
anywhere in it can obtain a value bounded by `Network`.
