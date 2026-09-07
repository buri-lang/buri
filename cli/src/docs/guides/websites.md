# Build a website

A website is one binary with two entries. A worker answers the request: it
builds the tree, renders it to HTML, and sends a document with the state it
rendered from embedded in it. A page picks that document up: it takes the markup
over and reads the state back out, without rendering any of it again.

Both halves are the same `ui/node` tree and the same `main.buri`.
[User interfaces](./user-interfaces.md) is the tree; this page is what a server
does with one.

## Two outputs, two entries

```textproto schema=build
# cmd/site/BUILD.buri
binary {
    outputs: [
        { platform: WEB, entry: "main" },
        { platform: CLOUDFLARE_WORKER, entry: "fetch" },
    ]
}
```

That is two artifacts out of one build: `.buri/out/web/cmd/site/main.mjs` and
`.buri/out/cloudflare-worker/cmd/site/fetch.mjs`. Each entry is its own
dead-code root, so the page never carries the renderer the worker uses and the
worker never carries the page's half. Each entry is also checked against its own
platform's grants, which is what lets `main` bind `Ui: host.ui` beside a `fetch`
that cannot. [Build files](../reference/build/build-files.md) has the rules.

## The whole program

```buri repo=cli/tests/repositories/concurrency/website/repo package=//cmd/site role=entry
// A website: one binary, two entries, one tree.

from "core/effect" import { Alloc, Request, Response, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/json" import { ToJson };
from "core/net/http" import * as http;
from "core/str" import * as str;
from "ui/effect" import { Location, Ui, Watch };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };
from "ui/web" import * as web;

derive ToJson for Site;
/// What the page is rendered from. The worker sends it with the document, and
/// the page reads it back out.
struct Site {
    title: Str,
    visitors: Int,
}

fn site(): Site {
    Site { title: "Buri", visitors: 3 }
}

/// The whole site, as one function of the path.
fn page<C: Alloc>(ctx: C, path: Prop<Str>, state: Site): Node<C> {
    let visitors = str.format(ctx, "visitors: ${state.visitors}");
    ui.computed(fn(scope) => at(path.read(scope), state.title, visitors))
}

/// Routing: an ordinary match on the path.
fn at<C>(path: Str, title: Str, visitors: Str): Node<C> {
    match (path) {
        "/" => {
            ui.region(.Main, [], [
                ui.heading(1, .Const(title)),
                ui.text(.Const(visitors)),
            ])
        },
        "/about" => ui.region(.Main, [], [ui.heading(1, .Const("About"))]),
        _other => ui.region(.Main, [], [ui.text(.Const("no page here"))]),
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
        Ui: host.ui,
        Watch: host.watch,
        Location: host.location,
    };
    match (web.resume(ctx)) {
        .Err(why) => .Err(why),
        .Ok(_) => {
            let picked = web.state(ctx).withDefault("none");
            match (io.println(ctx, "resumed ${web.path(ctx)} ${picked}")) {
                .Ok(_written) => .Ok(()),
                .Err(_e) => .Err("the page has nowhere to print"),
            }
        },
    }
}

export fn fetch(request: Request): Response {
    let ctx = context {
        Alloc: host.alloc,
    };
    let state = site();
    http.html(
        ctx,
        web.shell(
            ctx,
            web.render(page(ctx, .Const(request.path()), state)),
            state.toJson(ctx),
        ),
    )
}
```

## What the worker sends

`GET /` answers `200 text/html; charset=utf-8`:

```html
<!doctype html>
<html>
<head><meta charset="utf-8" /></head>
<body><main><h1>Buri</h1>visitors: 3</main><script id="buri-state" type="application/json">{"title":"Buri","visitors":3}</script></body>
</html>
```

`render` is the renderer `mount` uses, pointed at a document the runtime
supplies rather than at a browser's. There is no second renderer, so what the
worker writes is what the page would have built. It takes no context and cannot
need one: every constructor in `ui/node` is unbounded in `C`, so nothing in a
tree can act while it is being written out, and a handler is never called —
what comes back is text.

`shell` writes the document around it. The state goes in a
`<script id="buri-state">`, which a browser neither runs nor renders, and every
`<` in it is escaped, so a string holding `</script>` closes nothing. The
stylesheet the compiler extracted goes in the head, so the page the reader sees
first is already styled.

## Routing is a match

There is no route table. `page` takes the path as a `Prop<Str>` and matches on
it, and the two callers differ in one argument:

- the worker passes `.Const(request.path())`, which reads once and registers
  nothing;
- the page passes `web.route(ctx)`, which is the address bar as a cell.

`ui.computed` is what makes the difference matter. It re-runs when something it
read has changed, so navigating re-renders the subtree that read the path and
nothing else. That is `Watch`: the closure gets a `Scope`, which reads the graph
and can do nothing else, which is what makes it safe to re-run whenever the
runtime likes.

Text is rendered outside the closure, in `page`, because a `Scope` cannot
allocate. Anything the route needs as a string is prepared where there is a
context and captured.

## What the page does

`web.resume(ctx)` takes over the document the worker sent. It renders nothing
and removes nothing — the markup the reader is looking at is the markup that
arrived — and it picks up the embedded state, which `web.state(ctx)` answers as
the JSON text `shell` was given. Read it back with `json.parse` and
`json.decode`, the same pair the worker encoded it with.

`web.path(ctx)` is the path now. It is the same cell `route` wraps, so a page
that only wants to know where it is need not build a prop for it.

## Location is the page's alone

| Effect | Granted on |
|---|---|
| `Location` | `WEB` |

A worker has no address bar. It is handed a request and reads the path off that,
which is `Request.path` and no authority at all. So a `fetch` that asks for one
is refused on the line that asked:

```text
$ buri build //cmd/site
error: `location` implements `Location`, which is not allowed on the CLOUDFLARE_WORKER platform [effect-not-on-platform]
  --> cmd/site/main.buri:73:24
   |
73 |         Location: host.location,
   |                        ^^^^^^^^
   |
   = a platform is the set of effects its host exports; only a page has an address bar; a worker reads the path off the request it was handed
   = fix: drop `Location` from the context, or build this target for a platform that grants it: WEB
```

## Next

- [User interfaces](./user-interfaces.md) — the tree, signals, styles and
  themes.
- [Build a web server](./web-server.md) — the other way to answer a request,
  with a port of your own.
- [The standard library](../reference/standard-library.md) — `ui/web`'s five
  functions, and everything under them.
