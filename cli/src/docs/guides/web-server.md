# Build a web server

Being a server is three authorities, and a program names the ones it uses.
`Listen` accepts connections. `Sockets` pushes on one somebody else accepted.
`Net` — `core/net/http`'s effect — talks *out* to other servers. They are three
rather than one because a program that answers requests need not be one that can
make them, and a context is where that is written down.

`core/net/server` is the accepting half; `core/net/http` is the client half and
the place `Request` and `Response` are documented. There is one shape for an HTTP
message here, so a handler answers with the same `Response` a client reads, built
by the same `http.text`, `http.json` and `http.status`.

## Two routes and a JSON body

```textproto schema=build
# cmd/server/BUILD.buri
binary {
    outputs: [
        { platform: MACOS, arch: ARM64 },
    ]
}
```

`LINUX` and `MACOS` are the platforms that grant `Listen`, and `outputs` is
where a binary says which it is for. One that declares none builds for JS, which
grants neither `Listen` nor `Sockets` — see
[what refuses to serve](#what-refuses-to-serve).

```buri
// cmd/server/main.buri
from "core/effect" import { Alloc, Listen, Request, Response, Tasks };
from "core/host" import * as host;
from "core/json" import * as json;
from "core/json" import { Json };
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "core/str" import * as str;

/// One request, answered. An ordinary function over an ordinary context.
export fn route<C: Alloc>(ctx: C, request: Request): Response {
    match (request.path()) {
        "/health" => {
            let body = Json.Object([
                ("status", .Str("ok")),
                ("routes", .Array([.Str("/health"), .Str("/hello")])),
            ]);
            http.json(ctx, json.stringify(ctx, body))
        },
        "/hello" => http.text(ctx, str.format(ctx, "hello, ${request.query()}")),
        _ => http.status(404),
    }
}

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Listen: host.listen,
        Tasks: host.tasks,
    };
    server
        .serve(ctx, server.Server { port: 3000, onRequest: route })
        .mapErr(server.errorText)
}
```

```text
$ buri build //cmd/server
.buri/out/macos-arm64/cmd/server/server (2031008 bytes)
$ buri run //cmd/server &
$ curl -i http://127.0.0.1:3000/health
HTTP/1.1 200 OK
content-type: application/json
content-length: 45
connection: close

{"status":"ok","routes":["/health","/hello"]}
$ curl -s "http://127.0.0.1:3000/hello?name=buri"
hello, name=buri
$ curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:3000/nope
404
```

`request.path()` is the URL's path with neither query nor fragment, and it is
pure — asking a request where it is going costs no allocation. `request.query()`
is everything after the first `?`. Routing is an ordinary `match`, and `route` is
an ordinary function: nothing about it knows it is a handler.

`Tasks` is in the bound because `run` fans the accept loop out over
`listener.handlers` workers rather than driving one. Handlers running at the same
time is authority like any other, so a program that grants `Listen` and not
`Tasks` does not compile.

Every field of `Server` but `port` and `onRequest` is an `Option` the literal may
leave out — the address, the protocols, a certificate, a request limit, an idle
timeout, a drain deadline, the WebSocket hooks, a socket buffer. Leaving one out
is not choosing a default; it is declining to choose, and the runtime picks.
`buri docs core/net/server` has the table of what this one picks, and
`bind` and `run` are `serve`'s two halves for a program that needs the port
number before it answers anything.

## State that outlives a request

A handler answers and returns, so anything it has to remember lives behind a
mailbox. An actor is a value — an initial state and a step — and `start` gives it
one and answers an `Address` a handler may capture, because an address holds no
context of its own.

```buri name=counting
# from "core/actor" import * as actor;
# from "core/actor" import { Actor, Address, Reply };
# from "core/effect" import { Alloc, Request, Response, Sockets, Tasks };
# from "core/json" import * as json;
# from "core/json" import { Json };
# from "core/net/http" import * as http;
# from "core/net/server" import { Socket };
# from "core/str" import * as str;

/// The counter's protocol. A variant carrying no `Reply` is a `send`; one
/// carrying a `Reply<R>` is an `ask` that yields an `R`.
enum Hits {
    Seen,
    Total(Reply<Int>),
}

fn hits<C: Alloc + Tasks>(): Actor<C, Int, Hits> {
    Actor {
        state: 0,
        step: fn(c, seen, message) => {
            match (message) {
                .Seen => seen + 1,
                .Total(reply) => {
                    let _ = reply.answer(c, seen).ignore();
                    seen
                },
            }
        },
    }
}

fn route<C: Alloc + Tasks>(
    ctx: C,
    counted: Address<C, Int, Hits>,
    request: Request,
): Response {
    let _ = counted.send(ctx, .Seen).ignore();
    match (request.path()) {
        "/health" => {
            let seen = counted.ask(ctx, fn(reply) => .Total(reply)).withDefault(0);
            let body = Json.Object([
                ("status", .Str("ok")),
                ("served", .Num(seen.toF64())),
            ]);
            http.json(ctx, json.stringify(ctx, body))
        },
        "/hello" => http.text(ctx, str.format(ctx, "hello, ${request.query()}")),
        _ => http.status(404),
    }
}

/// Tells every socket in a room the same thing. `Sockets` and nothing else, so
/// it needs no listener and no port.
fn broadcast<C: Sockets>(ctx: C, room: [Socket], said: Str): () {
    room.foldCtx(ctx, fn(c, _sofar, one) => one.send(c, .Text(said)), ())
}
```

`main` gains two lines: `let counted = actor.start(ctx, hits());` before the
call, and `onRequest: fn(c, request) => route(c, counted, request)` in the
`Server` literal.

```text
$ curl -s "http://127.0.0.1:3000/hello?name=buri"
hello, name=buri
$ curl -s http://127.0.0.1:3000/health
{"status":"ok","served":2}
$ curl -s http://127.0.0.1:3000/health
{"status":"ok","served":3}
```

`send` and `ask` both answer a `Result` — `.Err(.Stopped)` once the actor has
stopped — and a handler that cannot act on a stopped counter drops it with
`ignore`, which [`discarded-result`](../reference/lints/discarded-result.md)
reports so that every such decision is in one list. [Tasks and
actors](./concurrency.md) is the rest of the model.

A `Server` with a `websocket` field speaks WebSockets, and the upgrade is
invisible: `onOpen` answers what the socket carries, every later hook is handed
it, and `onMessage` answers the next — so per-socket state is a value rather than
a table keyed by socket, and an actor's address is a good thing for it to be. A
`Socket` is inert, which is what makes that work: one integer, copyable, and
sendable to an actor that can push on it long after the request that opened it
returned. `broadcast` above is the shape — `Sockets` and nothing else, no
listener and no port. The hooks are in
[the standard library](../reference/standard-library.md).

## Stopping

`SIGTERM` and `SIGINT` do not kill a program holding a port. The platform stops
accepting, lets the requests in flight be answered, and then tells the accept
loop the listener is closed — so `serve` returns `.Ok(())`, `main` falls off its
own end, and whatever a program does after `serve` still happens:

```text
$ ./.buri/out/macos-arm64/cmd/server/server &
$ curl -s http://127.0.0.1:3000/health
{"status":"ok","served":1}
$ kill -TERM %1
$ wait %1; echo $?
0
```

A second signal is the operating system's own, so `Ctrl-C` twice still stops a
process that will not drain, and a program holding no port keeps the ordinary
behaviour.

## Testing a handler

A handler is a function of a context and a request, so a test calls it. Nothing
here binds `Listen`, opens a port, or starts a server:

```buri role=test use=counting
from "core/host/testing" import { alloc, sockets, tasks };
from "core/testing/assert" import * as assert;

test "an unknown path is a 404" {
    let ctx = context {
        Alloc: alloc(),
        Tasks: tasks(),
    };
    let counted = actor.start(ctx, hits());
    let answer = route(ctx, counted, http.request(.Get, "http://localhost/nope"));
    assert.eq(answer.status, 404);
}

test "a broadcast reaches every socket in the room" {
    let pushes = sockets();
    let ctx = context {
        Alloc: alloc(),
        Sockets: pushes,
    };
    let one = pushes.open();
    let two = pushes.open();
    broadcast(ctx, [one, two], "closing time");
    assert.eq(pushes.sent(), [
        (one, .Text("closing time")),
        (two, .Text("closing time")),
    ]);
}
```

Mark the three functions `export` and the suite reaches them through the
binary's entry point — `from "//cmd/server/main.buri" import { broadcast, hits,
route };` — which is the whole of a binary's surface. [Testing your
code](./testing.md) is the rest of it.

```text
$ buri test //cmd/server
3 passed, 0 failed, 0 skipped (0.3s)
```

Two of the three authorities have a double in `core/host/testing`, and the third
deliberately does not:

| | |
|---|---|
| `sockets()` | `open()` mints a `Socket` with no network behind it, `sent()` reads back `[(Socket, Message)]`, and `isOpen(socket)` says whether it is still one this double accepts. A fresh double per call, so what one test opens is invisible to the next |
| `tasks()` | Program order by default, then `anyOrder()`, `seed(n)`, `everyOrder()` and `faults([...])` — the double whose subject is scheduling rather than state |
| `Listen` | **No double.** What a fake acceptor answers is the test's own decision, so it is a struct with an `impl Listen`, written where it is needed |

The asymmetry is not an omission. A hand-written `Sockets` could record nothing —
an effect method takes only `self`, and `self` is immutable — so the recording
half has to be a handle into runner-side state, and the deciding half does not.

## What refuses to serve

| Effect | Granted on |
|---|---|
| `Listen`, `Sockets` | `LINUX`, `MACOS` |
| `Tasks` | `LINUX`, `MACOS`, `JS` |

Under `platform: WEB` this program is refused twice, on the two lines that
asked:

```text
$ buri build //cmd/server
error: `listen` implements `Listen`, which is not allowed on the WEB platform [effect-not-on-platform]
  --> cmd/server/main.buri:57:22
   |
57 |         Listen: host.listen,
   |                      ^^^^^^
   |
   = a platform is the set of effects its host exports; holding a port open is a native program's authority; a page is served rather than serving, and its host has no way to accept a connection
   = fix: drop `Listen` from the context, or build this target for a platform that grants it: LINUX, MACOS
error: `tasks` implements `Tasks`, which is not allowed on the WEB platform [effect-not-on-platform]
  --> cmd/server/main.buri:59:21
   |
59 |         Tasks: host.tasks,
   |                     ^^^^^
   |
   = a platform is the set of effects its host exports; `parallel` returns only when the last task has finished, which freezes a page; a page's concurrency is its event loop, and the effect that reaches it lands with the servers
   = fix: drop `Tasks` from the context, or build this target for a platform that grants it: LINUX, MACOS, JS
```

Each entry of `outputs` is checked against the whole graph separately, so a
binary can pass for MACOS and fail for JS — [compile to
JavaScript](./compile-to-js.md) is that half.

## Next

- [Tasks and actors](./concurrency.md) — `parallel`, actors as values, and what
  bounds a step.
- [Effects and capabilities](./effects.md) — why a context is where a program's
  authority is written.
- [The standard library](../reference/standard-library.md) — every `Server`
  field, the WebSocket hooks, and the drain.
