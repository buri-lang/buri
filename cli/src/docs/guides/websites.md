# Build a website

A website is one binary with two entries. A worker answers the request: it
builds the tree, renders it to HTML, and sends a document with the state it
rendered from embedded in it. A page picks that document up: it reads the state
back out, builds the same tree from it, and resumes on the markup that arrived —
nothing is rendered again, and the buttons work.

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
from "core/json" import * as json;
from "core/json" import { FromJson, ToJson };
from "core/net/http" import * as http;
from "core/str" import * as str;
from "ui/effect" import { Event, Location, Ui, Watch };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };
from "ui/signal" import { signal };
from "ui/web" import * as web;

derive FromJson, ToJson for Site;
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
///
/// The label and the handler are parameters because they are the halves only a
/// page has: the worker passes a constant and a handler that does nothing, and
/// the page passes a signal and one that writes it. Everything else is the same
/// tree on both sides, which is what makes the markup match.
fn page<C>(
    path: Prop<Str>,
    title: Str,
    visitors: Str,
    label: Prop<Str>,
    onPress: fn(C, Event) => (),
): Node<C> {
    ui.computed(fn(scope) => at(path.read(scope), title, visitors, label, onPress))
}

/// Routing: an ordinary match on the path.
fn at<C>(
    path: Str,
    title: Str,
    visitors: Str,
    label: Prop<Str>,
    onPress: fn(C, Event) => (),
): Node<C> {
    match (path) {
        "/" => {
            ui.region(.Main, [], [
                ui.heading(1, .Const(title)),
                ui.text(.Const(visitors)),
                ui.button(label, onPress),
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
    // The state the worker sent, read back before anything is built: the tree
    // the page resumes with is the tree the worker rendered, and this is what
    // it was rendered from.
    let sent = web.state(ctx).withDefault("null");
    let state = json
        .decode(ctx, json.parse(ctx, sent).withDefault(.Null))
        .withDefault(Site { title: "no state", visitors: 0 });
    let thanks = signal(ctx, "say thanks");
    let tree = page(
        web.route(ctx),
        state.title,
        str.format(ctx, "visitors: ${state.visitors}"),
        .Cell(thanks),
        fn(c, _event) => thanks.set(c, "thanks"),
    );
    match (web.resume(ctx, tree)) {
        .Err(why) => .Err(why),
        .Ok(_) => {
            match (io.println(ctx, "resumed ${web.path(ctx)} ${sent}")) {
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
            web.render(
                page(
                    .Const(request.path()),
                    state.title,
                    str.format(ctx, "visitors: ${state.visitors}"),
                    .Const("say thanks"),
                    fn(_ctx, _event) => (),
                ),
            ),
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
<body><main><h1>Buri</h1>visitors: 3<button type="button">say thanks</button></main><script id="buri-state" type="application/json">{"title":"Buri","visitors":3}</script></body>
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

Text is rendered outside the closure, in `main` and in `fetch`, because a
`Scope` cannot allocate: a closure may not capture a capability, so turning an
interpolation into a `Str` has to happen where there is a context. Prepare what
varies there and capture it. `state.toJson(ctx)` names a context for the same
reason — rendering JSON allocates.

The label and the handler are parameters of `page` for a different reason: they
are the halves only a page has. A handler writes a signal, which needs `Ui`, and
a worker's context grants none — so the worker passes a constant and a handler
that does nothing, and the page passes a signal and one that writes it. The
markup is the same either way, which is what a resume needs.

## What the page does

`web.state(ctx)` answers the JSON text `shell` was given. Read it back with
`json.parse` and `json.decode` — the pair the worker encoded it with — and build
the tree out of what comes back. That tree is what the page hands `resume`.

`web.resume(ctx, tree)` takes the document over. It creates no element and no
run of text: the renderer walks the tree against the markup that arrived, takes
the node already sitting where each one belongs, and adds what markup cannot
carry — the listeners, and the computations that re-run when a signal changes.
So the button the *server* wrote works on the first press, and the reader never
sees the page rebuilt.

It is the one renderer doing this, not a second one that reads markup, which is
why what a resume expects is exactly what a mount would have built.

`web.path(ctx)` is the path now. It is the same cell `route` wraps, so a page
that only wants to know where it is need not build a prop for it.

## A tree the markup does not match

A resume fails where the tree and the document disagree — a page resumed at an
address the server did not render, or built from a state it did not send:

```text
this page is not the markup the server sent: wanted <button>, found nothing left
```

That is the `.Err` from `resume`, and the page above turns it into `main`'s. A
resume that guessed at the difference would leave the reader looking at both
answers, so it stops and says which node it wanted. Text is the exception: a run
that differs is written, because the numbers a page renders come from a state
that is allowed to have moved on.

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
  --> cmd/site/main.buri:103:24
    |
103 |         Location: host.location,
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
