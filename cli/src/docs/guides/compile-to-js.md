# Compile to JavaScript

A binary's `outputs` say what it produces. `platform: "node"` is a module for
node or bun, `platform: "web"` is a page, and both emit JavaScript. They are two
platforms rather than two modes of one because they grant different effects.

## A module for node or bun

```textproto schema=build
# cmd/web/BUILD.buri
binary {
    dependencies: ["//lib/ledger", "//lib/money"]
    tags: ["client"]

    outputs: [
        { platform: "node" },
    ]
}
```

```text
$ buri build //cmd/web
.buri/out/node/cmd/web/web.mjs (3543 bytes)
```

The artifact is one self-contained ES module:

```text
$ node .buri/out/node/cmd/web/web.mjs
basket total: $36.50
```

`buri run //cmd/web` does the same through the toolchain, resolving `bun` or
`node` from `PATH`, or from `BURI_JS` naming one. You need nothing else: no
`package.json`, no bundler, no runtime dependency to install.

A binary that declares no `outputs` at all builds `node`, which is why
`buri run` works in a fresh `buri init` repository.

## Alongside a native binary

`outputs` is a list, and each entry is a separate artifact checked separately
against the whole graph:

```textproto schema=build
binary {
    outputs: [
        { platform: "native", variant: "linux-x86_64" },
        { platform: "node", entries: [
            { name: "main", function: "mainForNode" },
        ] },
    ]
}
```

Each platform hands its entry its own host, so each output enters through a
function of its own, and both call one function that takes `ctx`:

```buri ignore why="`mainForNode` is an entry only where a build file names it, and a fence has no build file"
# from "core/io" import * as io;
from "native" import { NativeHost };
from "node" import { NodeHost };
# from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    run(context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    })
}

export fn mainForNode(host: NodeHost): Result<(), Str> {
    run(context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    })
}

fn run<C: Allocator + Stdout>(ctx: C): Result<(), Str> {
    io.println(ctx, "hello").mapErr(fn(_e) => "could not write")
}
```

`buri build` produces both. `--output=node` picks one, and so does
`buri run --output=node`. `NodeHost` has no `listen` or `tcp`, so `mainForNode`
can't bind them.

## A page in a browser

`platform: "web"` takes no `variant`: there is no machine under a page.

```textproto schema=build
# cmd/basket/BUILD.buri
binary {
    sources: ["model.buri", "state.buri", "theme.buri", "view.buri"]
    dependencies: ["//lib/kit", "//lib/ledger", "//lib/money"]
    tags: ["client"]

    outputs: [
        { platform: "web" },
    ]
}
```

```text
$ buri build //cmd/basket
.buri/out/web/cmd/basket/main.mjs (59889 bytes)
$ ls .buri/out/web/cmd/basket/
index.html  main.css  main.mjs
```

Three files: the module, the styles the compiler extracted and deduped across
every package in the build, and the `index.html` that `web` ships, which links
the one and loads the other. Serve the directory. Writing the page is
[user interfaces](./user-interfaces.md).

## Another host

A JavaScript host the toolchain doesn't ship, like a Cloudflare Worker, is a
platform your repository writes. [Write your own platform](./custom-platforms.md)
builds one.

## Shipping part of it later

`core/lazy` splits a function, and everything only that function reaches, into a
file beside the artifact:

```buri
from "core/io" import * as io;
from "core/lazy" import * as lazy;
from "platform/effect" import { Stdout };

fn editor<C: Stdout>(ctx: C): () {
    io.println(ctx, "editing").ignore()
}

fn open<C: Stdout>(ctx: C, wanted: Bool): () {
    if (wanted) {
        let page = lazy.load(editor);
        page(ctx)
    } else {
        io.println(ctx, "reading").ignore()
    }
}
```

```text
$ ls .buri/out/web/cmd/basket/
index.html  main.0.mjs  main.css  main.mjs
```

`main.0.mjs` is the editor. The page fetches it when it reaches the `load` and
not before, so a reader who never opens the editor never downloads it. The name
is derived from the module's own URL at run time — nothing configures it, and
serving the directory is still all there is to do.

`load` takes the name of a function, and a native build ignores it and hands the
function straight back.

## What changes about the program

**The effects `main` may ask for.** A platform's host type *is* the set of
effects it offers. `WebHost` has no `fs`, `stdin`, `env` or `proc`, and has `ui`
and `watch`, which no other host has. Ask for one a platform does not offer and
you get `unknown-field` on the line that asked, with a note naming the
platforms that do.

**Nothing else.** No source file changes meaning across platforms, because there
is no conditional compilation. Numbers included: an `Int` is an `I64`
everywhere, and on this backend an `I64` is a `BigInt`, so a value past 2^53
keeps every digit.

If a library must not reach a JavaScript output at all, say so with a tag:
[enforce policy with tags](./tags-policy.md). The fields are in
[`build-files.md`](../reference/build/build-files.md), and the platform rules in
[`tags.md`](../reference/build/tags.md).
