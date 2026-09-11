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

from "core/effect" import { Allocator, Request, Response, Stdout };
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
/// The label and the two handlers are parameters because they are the halves
/// only a page has: the worker passes a constant and handlers that do nothing,
/// and the page passes a signal, one handler that writes it and one that
/// navigates. Everything else is the same tree on both sides, which is what
/// makes the markup match.
///
/// Only the last child reads the path, so only the last child is rebuilt when
/// the reader navigates. The button above it keeps the signal it is bound to.
fn page<C>(
    path: Prop<Str>,
    title: Str,
    visitors: Str,
    label: Prop<Str>,
    onPress: fn(C, Event) => (),
    onGo: fn(C, Event) => (),
): Node<C> {
    ui.region(.Main, [], [
        ui.heading(1, [], .Const(title)),
        ui.text(.Const(visitors)),
        ui.button(label, [], [], onPress, .Const(false), .Const(false)),
        ui.button(.Const("about"), [], [], onGo, .Const(false), .Const(false)),
        ui.computed(fn(scope) => at(path.read(scope))),
    ])
}

/// Routing: an ordinary match on the path.
fn at<C>(path: Str): Node<C> {
    match (path) {
        "/" => ui.region(.Article, [], [ui.text(.Const("home"))]),
        "/about" => ui.region(.Article, [], [ui.heading(2, [], .Const("About"))]),
        _other => ui.region(.Article, [], [ui.text(.Const("no page here"))]),
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
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
        fn(c, _event) => web.navigate(c, "/about"),
    );
    match (web.resume(ctx, tree)) {
        .Err(why) => .Err(why),
        .Ok(_) => {
            // A redirect. `/index.html` is not a page here, so the entry is
            // replaced rather than pushed: Back would otherwise take the reader
            // to the address they were just sent away from.
            let _ = if (web.path(ctx) == "/index.html") {
                web.replace(ctx, "/")
            } else {
                ()
            };
            match (io.println(ctx, "resumed ${web.path(ctx)} ${sent}")) {
                .Ok(_written) => .Ok(()),
                .Err(_e) => .Err("the page has nowhere to print"),
            }
        },
    }
}

export fn fetch(request: Request): Response {
    let ctx = context {
        Allocator: host.alloc,
    };
    let state = site();
    http.html(
        ctx,
        web.shell(
            ctx,
            request.path(),
            web.Document { ..web.defaultDocument(), title: state.title },
            web.render(
                page(
                    .Const(request.path()),
                    state.title,
                    str.format(ctx, "visitors: ${state.visitors}"),
                    .Const("say thanks"),
                    fn(_ctx, _event) => (),
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
<html lang="en">
<head><meta charset="utf-8" /><meta name="viewport" content="width=device-width, initial-scale=1" /><title>Buri</title></head>
<body><main><h1>Buri</h1>visitors: 3<button type="button">say thanks</button><button type="button">about</button><article>home</article></main><script id="buri-state" type="application/json" data-path="/">{"title":"Buri","visitors":3}</script></body>
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
`<` in it is escaped, so a string holding `</script>` closes nothing. The path it
was rendered for rides on the same script as `data-path`, and the stylesheet the
compiler extracted goes in the head, so the page the reader sees first is already
styled.

## The head is a value

`web.Document` is what the head says: `title` names the tab and `lang` names the
language. Build one from `defaultDocument` and write the fields this page
differs in. Routing is a match, so a page's title is a match too:

```buri
# from "ui/web" import * as web;

fn document(path: Str): web.Document {
    let title = match (path) {
        "/about" => "About — Buri",
        _other => "Buri",
    };
    web.Document { ..web.defaultDocument(), title }
}
```

Hand that to `shell` beside the tree the same match built, and each route names
its own tab.

The page's half is `web.title`, because the `.html` a WEB output writes carries
the artifact's name and nothing about the route:

```buri
# from "ui/effect" import { Location, Ui };
# from "ui/web" import * as web;

fn name<C: Location + Ui>(ctx: C): () {
    let path = web.route(ctx);
    web.title(
        ctx,
        .Computed(fn(scope) => {
            match (path.read(scope)) {
                "/about" => "About — Buri",
                _other => "Buri",
            }
        }),
    )
}
```

The title is a `Prop<Str>`, so it follows the route the way the tree does: a
computation over `route` is rewritten on every navigation, and a `.Const` names
the tab once.

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

Put it around as little as it can go around. Above the `computed` in `page` sit
the heading, the visitor count and the two buttons, and none of them reads the
path — so a navigation leaves those nodes exactly where they are, listeners and
all.

Text is rendered outside the closure, in `main` and in `fetch`, because a
`Scope` cannot allocate: a closure may not capture a capability, so turning an
interpolation into a `Str` has to happen where there is a context. Prepare what
varies there and capture it. `state.toJson(ctx)` names a context for the same
reason — rendering JSON allocates.

The label and the handlers are parameters of `page` for a different reason: they
are the halves only a page has. A handler writes a signal, which needs `Ui`, and
a worker's context grants none — so the worker passes a constant and handlers
that do nothing, and the page passes a signal, one handler that writes it and
one that navigates. The markup is the same either way, which is what a resume
needs.

## The page navigates itself

`web.navigate(ctx, path)` goes somewhere without loading a document. It puts the
address in the address bar and writes the cell `route` wraps, so what re-renders
is the subtree that read the path:

```buri ignore why="the handler `page` takes, out of the program above"
fn(c, _event) => web.navigate(c, "/about")
```

Everything else stays. The tree is the tree the reader is already looking at, so
every signal in the program keeps its value — a store of signals survives its own
navigation, which is the thing a `ui.link` cannot do: a link is a full document
load, and a full document load builds the app again from nothing.

`web.replace(ctx, path)` is the same, in place of the entry the reader is on
rather than beside it. That is what a redirect wants:

```buri ignore why="the redirect in `main`, out of the program above"
let _ = if (web.path(ctx) == "/index.html") { web.replace(ctx, "/") } else { () };
```

Push and the reader can press Back to where they were. Replace and they cannot —
which is right here, because Back onto `/index.html` would only send them
forward again.

Both need `Location` **and** `Ui`: one to move the address bar, one to write the
cell. Reaching another site is a `ui.link`, and it should be — a reader deserves
to see where a link goes.

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

## The address is checked first

`shell` writes the path it rendered for beside the state, and `resume` compares
it against the address bar before it walks a single node:

```text
this page is not the page the server sent: it was rendered at / and the address is /about
```

Without that, two routes that render the same *shape* resume into each other and
nothing notices — a `/` and an `/about` that are both a heading and a paragraph
adopt each other's markup happily, and the reader ends up looking at one page
with the other page's handlers on it.

A query string and a fragment are not part of a path and are not compared.
`Request.path` leaves both out and so does the address bar, so `/notes?page=2`
and `/notes#top` are the same page as `/notes`.

Markup no `shell` wrote carries no path, and then only the shape below is
checked.

## A tree the markup does not match

A resume also fails where the tree and the document disagree — a page built from
a state the server did not send:

```text
this page is not the markup the server sent: wanted <button>, found nothing left
this page is not the markup the server sent: <main> holds more than the tree does
```

Both are the `.Err` from `resume`, and the page above turns it into `main`'s.
The first is a node the tree wanted and the markup does not have; the second is
the same disagreement from the other end, a node the markup has and the tree
never accounted for. A resume that guessed would leave the reader looking at
both answers, so it stops and names what it found.

Text and attributes are the exception: a run of text or an attribute that
differs is written rather than refused, because the numbers a page renders come
from a state that is allowed to have moved on. What is refused is the *shape* —
an element that is not there, one that is not the same element, or one the tree
never accounted for.

Resume once. A second `resume` on the same document walks markup the first one
has already taken over, so it answers the same `.Err`.

A worker's tree holds no `field` and no `toggle`. Both bind to a `Signal`, and
only `Ui` makes one — which a worker does not have. Render the shell of a form
on the server and build the fields on the page, or make the whole form a page
that mounts rather than resumes.

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

## Look at it locally

`buri run` builds the page and serves it, so there is nothing to write and
nothing to install:

```text
$ buri run //cmd/site
serving //cmd/site on http://127.0.0.1:4000/
```

Files under the artifact directory are answered as themselves and every other
path is answered with the entry shell — so `/about` arrives with `/about` in the
address bar, and the match above sees it. `--watch` rebuilds on a save, and
nothing is cached, so a reload is the new build.
[`buri run`](../reference/cli/run.md) has the port, the flags and the rest.

What it serves is the shell the compiler wrote, not the document `fetch`
renders. A `main` that resumes therefore finds markup no `shell` wrote and says
so: this is how you look at a page that *mounts*. The resumed page is the
worker's, and the worker runs on its platform's own local runner.

## Next

- [User interfaces](./user-interfaces.md) — the tree, signals, styles and
  themes.
- [Build a web server](./web-server.md) — the other way to answer a request,
  with a port of your own.
- [The standard library](../reference/standard-library.md) — every function
  `ui/web` exports, and everything under them.
