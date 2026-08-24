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
  kit/                        //lib/kit          tagged `client`, owns design tokens
    BUILD.buri
    lib.buri
    tokens.buri
    card.buri
    test/
      card.buri
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
  basket/                     //cmd/basket        one WEB output: a page
    BUILD.buri
    main.buri
    model.buri
    state.buri
    theme.buri
    view.buri
    test/
      model.buri
      view.buri
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

//cmd/basket  (web · client)
  ├─ //lib/kit           tags: client
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

`//lib/kit` is the mirror image: tagged `client`, and `forbids` is symmetric, so
one line in `REPO.buri` keeps it out of `//cmd/server` from both directions.
Everything above the tier tags — `//lib/money`, `//lib/ledger` — is untagged and
links into all four binaries, which is what an untagged library is *for*.

## What each file is there to show

| File | The point |
|---|---|
| [`REPO.buri`](./REPO.buri) | The whole tag vocabulary with its policy attached, private-by-default visibility, and how little else belongs in a repository-wide file |
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
| [`lib/kit/tokens.buri`](./lib/kit/tokens.buri) | A package's own design-token vocabulary, and the one function only that package can write |
| [`lib/kit/card.buri`](./lib/kit/card.buri) | Components as plain functions of no context at all, the static style tier, and the one place the computed tier earns its keep |
| [`cmd/server/main.buri`](./cmd/server/main.buri) | The effect budget as the context `main` builds, and re-exporting for the test suite |
| [`cmd/web/BUILD.buri`](./cmd/web/BUILD.buri) | The tag error, spelled out, and why dropping the tag does not avoid it |
| [`cmd/basket/BUILD.buri`](./cmd/basket/BUILD.buri) | Why `platform: WEB` is a different set of effects rather than a flag on a JavaScript output, and the three files it writes |
| [`cmd/basket/main.buri`](./cmd/basket/main.buri) | A page's effect budget, and one theme per package whose tokens the program uses |
| [`cmd/basket/theme.buri`](./cmd/basket/theme.buri) | The contract between a library's tokens and an app, as a `match` that stops compiling |
| [`cmd/basket/view.buri`](./cmd/basket/view.buri) | Which of the three reactive constructors to reach for, and what each one re-runs |
| [`cmd/basket/test/view.buri`](./cmd/basket/test/view.buri) | A page tested with no browser: a `Fetch` written for the test, a keyed row's identity across a change, and a theme switch that moves no element |
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
- **Then compare all three `main.buri`s at once.** `//cmd/server` binds `Fs` and
  `Env`, `//cmd/web` binds neither, and `//cmd/basket` binds `Ui`, `Watch` and
  `Fetch` — which `core/host` exports under `platform: WEB` and under no other.
  None of the three would build for either of the others' outputs, and the error
  lands on the line that asked for the effect.
- **Follow a token from `lib/kit/tokens.buri` to a colour.** `Token.Surface` is
  a name //lib/kit chose; `cmd/basket/theme.buri` says it is worth this app's
  `Shade.Raised`; `day` and `night` say what *that* is worth; and `main.buri`
  hands both mappings to `mount`. Three files, one chain, resolved once — and
  the only thing holding it together is a `match` that stops compiling.
- **Notice how few `platforms` fields there are.** Two, both inside a `test`
  block, and both saying where a *suite* runs rather than what a library
  supports — a suite that renders a tree needs the reactive graph, which is a
  JavaScript-backend intrinsic. No library or binary rule names a platform at
  all. Libraries take no position on platforms unless they are doing something
  platform-specific; the one real restriction lives on the `server` tag, where
  it is policy rather than a fact about any single library.

## The page

`//cmd/basket` is a real application — a basket of ledger lines you can type
into, settle, and fill from the server's own `/entries` route — and it is here
for the same reason everything else is: it is the smallest thing that exercises
the whole of the interface vocabulary. Signals, a memo, a keyed list, a form,
both style tiers, dark mode, and a request that answers through a callback.

Building it writes three files rather than one:

```
.buri/out/web/cmd/basket/basket.mjs      the module
.buri/out/web/cmd/basket/basket.css      every static style, deduped across the build
.buri/out/web/cmd/basket/basket.html     a shell that links the one and loads the other
```

Open the `.html` and the page runs. Run the `.mjs` under `bun` or `node` and it
also runs: there is no document, so the runtime supplies one, which is what
makes a page something a test can render.

A component is an ordinary function returning a value. It takes no context, no
allocator and no authority, because building a tree is fixed-size construction —
so a component cannot do anything, and there is nothing to mock:

```buri pkg=//cmd/basket platform=WEB
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/signal" import { Signal };

from "//lib/kit" import { card };
from "//lib/ledger" import { Entry, total };

/// The running total, in a card.
///
/// `C` is unbounded because nothing here has a handler. The `Prop` is where the
/// reactivity is: when `lines` changes, this one string is rewritten and every
/// element around it is left exactly where it was.
///
/// The amount is rendered inside the closure because that is the only place it
/// can be. A reactive computation is handed a `Scope`, which grants reading the
/// signal graph and nothing else — so `Cents.format`, which allocates, is not
/// callable from one, and `Cents.parts`, which does not, is.
fn runningTotal<C>(lines: Signal<[Entry]>): Node<C> {
  card(.Const("Basket"), [
    ui.text(.Computed(fn(scope) => {
      let both = total(lines.get(scope)).parts();
      "\$${both.0}.${both.1}"
    })),
  ])
}
```

A library that uses design tokens declares its own closed vocabulary and styles
itself against that. The app closes the loop with one `match`:

```buri pkg=//cmd/basket platform=WEB
from "ui/style" import { Color };
from "ui/theme" import { Theme };

from "//lib/kit" import { themed, Token };

/// What //lib/kit's four tokens are worth here.
fn inBlue(t: Token): Color {
  match (t) {
    .Surface => .Rgb(255, 255, 255),
    .OnSurface => .Rgb(24, 24, 27),
    .Edge => .Rgb(228, 228, 231),
    .Accent => .Rgb(37, 99, 235),
  }
}

/// What `mount` is handed, one of these per package whose tokens are used.
fn kitTheme(): Theme {
  themed(inBlue)
}
```

That `match` is the whole contract, and it is checked the way every other
contract in this language is — by not compiling. Leave a token out and the day
//lib/kit adds a fifth one is the day this stops building, which is the only
moment the omission is still cheap to fix:

```buri fail code=match-not-exhaustive pkg=//cmd/basket platform=WEB
from "ui/style" import { Color };

from "//lib/kit" import { Token };

fn incomplete(t: Token): Color {
  match (t) {
    .Surface => .Rgb(255, 255, 255),
    .OnSurface => .Rgb(24, 24, 27),
    .Edge => .Rgb(228, 228, 231),
  }
}
```

## Its documentation is tested too

`buri docs test` compiles every fenced example in every markdown file of
whatever repository you run it in, against that repository's own packages.
Nothing has to be configured: a `repo=` on the fence is only needed when the
example lives in a *different* repository's documentation.

```buri run
from "core/effect" import { Alloc, Stdout };
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
