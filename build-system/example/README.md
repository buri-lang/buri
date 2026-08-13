# A worked monorepo

The snippets in the documents above come from these files, or from a
hypothetical change to them that the surrounding prose names. Nothing here is
compiled — there is no compiler — but everything is written to be consistent
with [`SPEC.md`](../../SPEC.md) and with itself.

```
REPO.buri                     repository root, toolchain pin, tag vocabulary
lib/
  money/                      //lib/money        no tags: links into anything
    BUILD.buri
    lib.buri
    cents.buri
    parse.buri
    test/
      cents.buri
      parse.buri
  ledger/                     //lib/ledger       a library with a subdirectory
    BUILD.buri
    lib.buri
    entry.buri
    posting/
      rules.buri
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
  server/                     //cmd/server:server   3 native outputs
    BUILD.buri
    main.buri
    routes.buri
    test/
      routes.buri
  web/                        //cmd/web:web         one JS output
    BUILD.buri
    main.buri
tools/
  report/                     a library and a binary in one package
    BUILD.buri
    lib.buri                  //tools/report
    render.buri
    main.buri                 //tools/report:report_cli
    flags.buri
    test/
      render.buri
      flags.buri
```

## The graph

```
//cmd/server:server  (linux/x86_64, linux/arm64, macos/arm64 · server)
  ├─ //lib/store         tags: server
  │    ├─ //lib/ledger
  │    └─ //lib/money
  ├─ //lib/ledger
  └─ //lib/money

//cmd/web:web        (js · client)
  ├─ //lib/ledger
  └─ //lib/money

//tools/report:report_cli  (linux/x86_64, macos/arm64 · server)
  ├─ //tools/report    (implicit: same package)
  ├─ //lib/ledger
  └─ //lib/money
```

`//lib/store` is reachable only from the server binary, and it is tagged
`server`, so it can never be reached from `//cmd/web:web` — not by a direct
edge, which visibility also forbids, and not by four hops through a library that
looked harmless, which visibility would not catch.

## What each file is there to show

| File | The point |
|---|---|
| [`REPO.buri`](./REPO.buri) | An exactly pinned toolchain, one custom dimension, one policy constraint, private-by-default visibility |
| [`lib/money/lib.buri`](./lib/money/lib.buri) | A complete public surface in two lines; `toCents` deliberately left off it |
| [`lib/money/cents.buri`](./lib/money/cents.buri) | Why a type's methods all live in one file, and what `opaque` hides from whom |
| [`lib/money/parse.buri`](./lib/money/parse.buri) | Free functions over a type you own but did not declare in this module; three levels of visibility in one file |
| [`lib/money/test/cents.buri`](./lib/money/test/cents.buri) | Tests that reach the library the way a dependent does, and the assertion they cannot write |
| [`lib/ledger/BUILD.buri`](./lib/ledger/BUILD.buri) | A subdirectory that is organization, not a boundary |
| [`lib/ledger/entry.buri`](./lib/ledger/entry.buri) | Why `total` is a free function and `add` is a method |
| [`lib/store/BUILD.buri`](./lib/store/BUILD.buri) | Visibility and tags side by side, doing two different jobs |
| [`lib/store/codec.buri`](./lib/store/codec.buri) | A dependency created by method resolution rather than by an import |
| [`lib/store/test/store.buri`](./lib/store/test/store.buri) | The test platform's in-memory `Fs`, and `test { data: ... }` |
| [`cmd/server/main.buri`](./cmd/server/main.buri) | A closed context on `main`, and re-exporting for the test suite |
| [`cmd/web/BUILD.buri`](./cmd/web/BUILD.buri) | The tag error, spelled out, and why dropping the tag does not avoid it |
| [`tools/report/BUILD.buri`](./tools/report/BUILD.buri) | Two rules, one directory, one build file |
| [`tools/report/main.buri`](./tools/report/main.buri) | A binary reaching its co-located library through `./lib` and nothing else |

## Things to try reading for

- **Open one `lib.buri` and try to name the library's API.** That is the whole
  test of the design: if the file is not enough, the boundary is not real.
- **Follow `toCents`.** Declared in `lib/money/cents.buri`, exported at module
  level, used by `lib/money/parse.buri`, absent from `lib/money/lib.buri`,
  unreachable from `lib/store`, and unmentionable in `lib/money/test/cents.buri`.
- **Count the ways `//lib/store` is kept out of the browser.** The tag, the
  visibility list, and the fact that its functions demand an `Fs` the JS
  platform cannot grant. Three mechanisms, three failure modes, one intent.
- **Compare `cmd/server/BUILD.buri` and `cmd/web/BUILD.buri`.** Same libraries,
  same sources, different configurations, and every difference between the two
  builds is visible in those two files.
