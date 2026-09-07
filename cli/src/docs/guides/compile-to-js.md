# Compile to JavaScript

A binary's `outputs` say what it produces. `platform: JS` is a module for node
or bun, `platform: WEB` is a page, and both emit JavaScript. They are two
platforms rather than two modes of one because they grant different effects.

## A module for node or bun

```textproto schema=build
# cmd/web/BUILD.buri
binary {
    dependencies: ["//lib/ledger", "//lib/money"]
    tags: ["client"]

    outputs: [
        { platform: JS, js { module: ESM } },
    ]
}
```

```text
$ buri build //cmd/web
.buri/out/js/cmd/web/web.mjs (3543 bytes)
```

The artifact is one self-contained ES module:

```text
$ node .buri/out/js/cmd/web/web.mjs
basket total: $36.50
```

`buri run //cmd/web` does the same through the toolchain, resolving `bun` or
`node` from `PATH`, or from `BURI_JS` naming one. You need nothing else: no
`package.json`, no bundler, no runtime dependency to install.

`module: ESM` is the field's only accepted value, and the compiler refuses
anything else where you wrote it.

A binary that declares no `outputs` at all builds for JS, which is why
`buri run` works in a fresh `buri init` repository.

## Alongside a native binary

`outputs` is a list, and each entry is a separate artifact checked separately
against the whole graph:

```textproto schema=build
binary {
    outputs: [
        { platform: LINUX, arch: X86_64 },
        { platform: JS, js { module: ESM } },
    ]
}
```

`buri build` produces both. `--output=js` picks one, and so does
`buri run --output=js`. The compiler checks the two independently, so a binary
can pass for Linux and fail for JS, because the JS host grants less.

## A page in a browser

`platform: WEB` takes no `arch` and no `js { module }`: a browser loads an ES
module and there is no other kind.

```textproto schema=build
# cmd/basket/BUILD.buri
binary {
    sources: ["model.buri", "state.buri", "theme.buri", "view.buri"]
    dependencies: ["//lib/kit", "//lib/ledger", "//lib/money"]
    tags: ["client"]

    outputs: [
        { platform: WEB },
    ]
}
```

```text
$ buri build //cmd/basket
.buri/out/web/cmd/basket/basket.mjs (59889 bytes)
$ ls .buri/out/web/cmd/basket/
basket.css  basket.html  basket.mjs
```

Three files: the module, the styles the compiler extracted and deduped across
every package in the build, and an HTML shell that links the one and loads the
other. Serve the directory. Writing the page is
[user interfaces](./user-interfaces.md).

## A worker

`platform: CLOUDFLARE_WORKER` is the other JavaScript artifact: a module the
platform *calls*, once per request, rather than a program that starts itself.
`entry` names the exported function it enters through.

```textproto schema=build
# cmd/site/BUILD.buri
binary {
    outputs: [
        { platform: WEB, entry: "main" },
        { platform: CLOUDFLARE_WORKER, entry: "fetch" },
    ]
}
```

```text
$ buri build //cmd/site
.buri/out/web/cmd/site/main.mjs (47662 bytes)
.buri/out/cloudflare-worker/cmd/site/fetch.mjs (35559 bytes)
```

Two entries out of one `main.buri`, and the platform fixes each one's signature:
a page is `fn main(): Result<(), Str>`, a worker is
`fn fetch(request: Request): Response`. The wrong shape is a type error.

Each entry is its own dead-code root, so the page carries nothing only the
worker reaches and the worker carries nothing only the page does. Each is
checked against its own platform's grants too, which is what lets `main` bind
`Ui: host.ui` beside a `fetch` that cannot.

The worker's module ends in `export default { fetch }` instead of the
self-starting epilogue every other JavaScript output gets. `buri run` never runs
one: there is nothing to start. Build it, and let the platform call it.
[Build a website](./websites.md) is both halves end to end.

## Shipping part of it later

`core/lazy` splits a function, and everything only that function reaches, into a
file beside the artifact:

```buri
from "core/effect" import { Stdout };
from "core/io" import * as io;
from "core/lazy" import * as lazy;

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
basket.0.mjs  basket.css  basket.html  basket.mjs
```

`basket.0.mjs` is the editor. The page fetches it when it reaches the `load` and
not before, so a reader who never opens the editor never downloads it. The name
is derived from the module's own URL at run time — nothing configures it, and
serving the directory is still all there is to do.

`load` takes the name of a function, and a native build ignores it and hands the
function straight back.

## What changes about the program

**The effects `main` may ask for.** A platform *is* the set of effects its host
exports. Under `WEB`, `core/host` exports no `fs`, `stdin`, `env` or `proc`, and
exports `ui` and `watch` there and nowhere else. Ask for one a platform does not
grant and you get `effect-not-on-platform` on the line that asked.
`buri docs error effect-not-on-platform` has the table.

**Nothing else.** No source file changes meaning across platforms, because there
is no conditional compilation. Numbers included: an `Int` is an `I64`
everywhere, and on this backend an `I64` is a `BigInt`, so a value past 2^53
keeps every digit.

If a library must not reach a JavaScript output at all, say so with a tag:
[enforce policy with tags](./tags-policy.md). The fields are in
[`build-files.md`](../reference/build/build-files.md), and the platform rules in
[`tags.md`](../reference/build/tags.md).
