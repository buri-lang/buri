# Compile to JavaScript

A binary's `outputs` say what it produces. `platform: JS` is a module for node
or bun; `platform: WEB` is a page. Both emit JavaScript, and they are two
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

The artifact is one self-contained ES module. Run it with whatever you have:

```text
$ node .buri/out/js/cmd/web/web.mjs
basket total: $36.50
```

`buri run //cmd/web` does the same through the toolchain, resolving `bun` or
`node` from `PATH`, or from `BURI_JS` naming one. Nothing else is needed —
there is no `package.json`, no bundler, and no runtime dependency to install.

An ES module is the only kind this toolchain emits. `module: ESM` says so
explicitly and is the field's only accepted value; anything else is refused
where it is written.

A binary that declares no `outputs` at all builds for JS, which is what makes
`buri run` work in a fresh `buri init` repository before anything has said where
the program ships.

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
`buri run --output=js`. The two are checked independently, so a binary can pass
for Linux and fail for JS — which is what you want, because the JS host grants
less.

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
other. Serve the directory. Writing the page itself is
[user interfaces](./user-interfaces.md).

## What changes about the program

**The effects `main` may ask for.** A platform *is* the set of effects its host
exports, so `core/host` exports no `fs`, `stdin`, `env` or `proc` under `WEB`,
and exports `ui` and `watch` under `WEB` and nowhere else. Asking for one a
platform does not grant is a compile error at the line that asked, reported as
`effect-not-on-platform` — `buri docs error effect-not-on-platform` has the
table, and the rule about which platforms a module is checked against.

**Nothing else.** No source file changes meaning across platforms; there is no
conditional compilation. Numbers included: an `Int` is an `I64` everywhere, and
on this backend an `I64` is a `BigInt`, so a value past 2^53 survives with every
digit.

If a library must not reach a JavaScript output at all, say so with a tag rather
than by convention — [enforce policy with tags](./tags-policy.md). The fields
themselves are in [`build-files.md`](../reference/build/build-files.md), and the
platform rules in [`tags.md`](../reference/build/tags.md).
