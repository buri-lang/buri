# A worked monorepo

This repository is the build system's worked example, and the source the
documentation's snippets are drawn from. All of it compiles: `buri build //...`
builds it, `buri test //...` runs its suites, and the examples in this file are
compiled by `buri docs test`.

It is also the smallest thing that exercises every rule the build system has —
visibility, tags, platforms, a testing surface, and a library boundary — which
is why the documentation keeps pointing at it.

```
REPO.buri                     repository root, tag vocabulary
lib/
  money/                      //lib/money        no tags: links into anything
    BUILD.buri
    lib.buri
    cents.buri
    parse.buri
    test/
      cents.buri
      parse.buri
  ledger/                     //lib/ledger       a subdirectory, and a testing surface
    BUILD.buri
    lib.buri
    entry.buri
    posting/
      rules.buri
    testing/
      lib.buri                //lib/ledger/testing — test-only, by its path
      fixtures.buri
    test/
      ledger.buri
  store/                      //lib/store        tagged `server`
    BUILD.buri
    lib.buri
    codec.buri
    file_store.buri
    test/
      store.buri
      golden/
        log.txt
cmd/
  server/                     //cmd/server        3 native outputs
    BUILD.buri
    main.buri
    routes.buri
    test/
      routes.buri
  web/                        //cmd/web           one JS output
    BUILD.buri
    main.buri
tools/
  report/                     a library and a binary in one package
    BUILD.buri
    lib.buri                  //tools/report
    render.buri
    main.buri                 //tools/report/main
    flags.buri
    test/
      render.buri
      flags.buri
```

## The graph

```
//cmd/server  (linux/x86_64, linux/arm64, macos/arm64 · server)
  ├─ //lib/store         tags: server
  │    ├─ //lib/ledger
  │    └─ //lib/money
  ├─ //lib/ledger
  └─ //lib/money

//cmd/web     (js · client)
  ├─ //lib/ledger
  └─ //lib/money

//tools/report (binary)  (linux/x86_64, macos/arm64 · server)
  ├─ //tools/report    (implicit: same package)
  ├─ //lib/ledger
  └─ //lib/money
```

`//lib/store` is reachable only from the server binary, and it is tagged
`server`, so it can never be reached from `//cmd/web` — not by a direct
edge, which visibility also forbids, and not by four hops through a library that
looked harmless, which visibility would not catch.

## What each file is there to show

| File | The point |
|---|---|
| [`REPO.buri`](./REPO.buri) | An exactly pinned toolchain, the whole tag vocabulary with its policy attached, private-by-default visibility |
| [`lib/money/lib.buri`](./lib/money/lib.buri) | A complete public surface in two lines; `toCents` deliberately left off it |
| [`lib/money/cents.buri`](./lib/money/cents.buri) | Why a type's methods all live in one file, and what an unexported field hides from whom |
| [`lib/money/parse.buri`](./lib/money/parse.buri) | Free functions over a type declared in another module; three levels of visibility in one file |
| [`lib/money/test/cents.buri`](./lib/money/test/cents.buri) | `test` and `assert`, a context built only where one is needed, and the assertion this suite cannot write |
| [`lib/ledger/BUILD.buri`](./lib/ledger/BUILD.buri) | A subdirectory that is organization, not a boundary, and a `testing` one that is a second surface |
| [`lib/ledger/testing/lib.buri`](./lib/ledger/testing/lib.buri) | Fixtures a library ships for other people's tests, unreachable from production by path |
| [`lib/ledger/entry.buri`](./lib/ledger/entry.buri) | Why `total` is a free function and `add` is a method |
| [`lib/store/BUILD.buri`](./lib/store/BUILD.buri) | Visibility and tags side by side, doing two different jobs |
| [`lib/store/codec.buri`](./lib/store/codec.buri) | A dependency created by method resolution rather than by an import |
| [`lib/store/test/store.buri`](./lib/store/test/store.buri) | `Hermetic`'s in-memory `Fs`, and `test { data: ... }` |
| [`cmd/server/main.buri`](./cmd/server/main.buri) | The effect budget as the context `main` builds, and re-exporting for the test suite |
| [`cmd/web/BUILD.buri`](./cmd/web/BUILD.buri) | The tag error, spelled out, and why dropping the tag does not avoid it |
| [`tools/report/BUILD.buri`](./tools/report/BUILD.buri) | Two rules, one directory, one build file |
| [`tools/report/main.buri`](./tools/report/main.buri) | A binary reaching its co-located library through `//tools/report` and nothing else |

## Things to try reading for

- **Open one `lib.buri` and try to name the library's API.** That is the whole
  test of the design: if the file is not enough, the boundary is not real.
- **Follow `toCents`.** Declared in `lib/money/cents.buri`, exported at module
  level, used by `lib/money/parse.buri`, absent from `lib/money/lib.buri`,
  unreachable from `lib/store`, and unmentionable in `lib/money/test/cents.buri`.
- **Count the ways `//lib/store` is kept out of the browser.** The tag, the
  visibility list, and the fact that its functions name `Fs` in their bounds —
  which the JS platform cannot satisfy. Three mechanisms, three failure modes,
  one intent.
- **Follow `sample()`.** Declared in `lib/ledger/testing/fixtures.buri`, built
  from the library's internals, re-exported by `lib/ledger/testing/lib.buri`,
  used by two suites in two packages, and importable by neither library's
  production sources.
- **Compare `cmd/server/BUILD.buri` and `cmd/web/BUILD.buri`.** Same libraries,
  same sources, different platforms and different tags, and every difference
  between the two builds is visible in those two files.
- **Notice how few `platforms` fields there are.** Exactly zero, in a repository
  that ships both a native binary and a JS module. Libraries take no position on
  platforms unless they are doing something platform-specific; the one real
  restriction lives on the `server` tag, where it is policy rather than a fact
  about any single library.

## Its documentation is tested too

`buri docs test` compiles every fenced example in every markdown file of
whatever repository you run it in, against that repository's own packages.
Nothing has to be configured: a `repo=` on the fence is only needed when the
example lives in a *different* repository's documentation.

```buri run
from "core/cap" import { Alloc, Stdout };
from "core/host" import * as host;

from "//lib/money" import { fromCents };

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc:  host.alloc,
    Stdout: host.stdout,
  };
  let _ = ctx.println("a latte costs ${fromCents(450).format(ctx)}");
  .Ok(())
}
```

```stdout
a latte costs $4.50
```

That block imports `//lib/money` — a package of this repository — and the test
suite compiles it, runs it, and compares what it printed with the transcript
above. A change to `format` that altered the output would fail here.
