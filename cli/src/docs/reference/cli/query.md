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

`path` is the one that earns its place: the answer to "why does the browser
build pull in the database layer" is an edge, and printing the edge is faster
than reading build files.
