## What it does

Answers questions about the build graph without building anything.

```text
deps(//cmd/server)              what it depends on, transitively
rdeps(//lib/money)              what depends on it
path(//cmd/web, //lib/store)    why — the edge chain, with the line that declares each
tags(//cmd/server)              every tag in its closure, and which target contributed it
platforms(//cmd/web)            the platforms its closure permits
sources(//lib/money)            the files the rule names
```

`path` is the one that earns its place. "Why does the browser build pull in the
database layer" is a question about an edge, and printing the edge beats reading
build files.
