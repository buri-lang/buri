# Build a web server

Being a server is three authorities, and a program names the ones it uses.
`Listen` accepts connections. `Sockets` pushes on one somebody else accepted.
`Net`, which is `core/net/http`'s effect, talks *out* to other servers. A
program that answers requests need not be one that can make them.

`core/net/server` is the accepting half; `core/net/http` is the client half, and
where `Request` and `Response` are documented. A handler answers with the same
`Response` a client reads, built by the same `http.text`, `http.json` and
`http.status`.

## Two routes and a JSON body

```textproto schema=build
# cmd/server/BUILD.buri
binary {
    outputs: [
        { platform: MACOS, arch: ARM64 },
    ]
}
```

`LINUX` and `MACOS` are the platforms that grant `Listen`. A binary that
declares no `outputs` builds for JS, which grants neither `Listen` nor
`Sockets`. See [what refuses to serve](#what-refuses-to-serve).

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
pure. `request.query()` is everything after the first `?`. Routing is an ordinary
`match`, and `route` is an ordinary function: nothing about it knows it is a
handler.

`Tasks` is in the bound because `run` fans the accept loop out over
`listener.handlers` workers. Handlers running at the same time is authority like
any other, so a program that grants `Listen` and not `Tasks` does not compile.

Every field of `Server` but `port` and `onRequest` is an `Option` the literal may
leave out: the address, the protocols, a certificate, a request limit, an idle
timeout, a drain deadline, the WebSocket hooks, a socket buffer. Leaving one out
declines to choose, and the runtime picks — `buri docs core/net/server` has the
table. `bind` and `run` are `serve`'s two halves, for a program that needs the
port number first.

## State that outlives a request

A handler answers and returns, so anything it has to remember lives behind a
mailbox. An actor is an initial state and a step; `start` gives it a mailbox and
answers an `Address` a handler may capture, because an address holds no context
of its own.

```buri name=counting
# from "core/actor" import * as actor;
# from "core/actor" import { Actor, Address, Stepped };
# from "core/effect" import { Alloc, Request, Response, Sockets, Tasks };
# from "core/json" import * as json;
# from "core/json" import { Json };
# from "core/net/http" import * as http;
# from "core/net/server" import { Socket };
# from "core/str" import * as str;

/// The counter's protocol: what a handler may send, and what it gets back.
enum Hits {
    Seen,
}

fn hits<C: Alloc + Tasks>(): Actor<C, Int, Hits, Int> {
    Actor {
        state: 0,
        step: fn(c, seen, message) => {
            match (message) {
                .Seen => Stepped { state: seen + 1, answer: seen + 1 },
            }
        },
    }
}

fn route<C: Alloc + Tasks>(
    ctx: C,
    counted: Address<C, Int, Hits, Int>,
    request: Request,
): Response {
    let seen = counted.sendMessage(ctx, .Seen).withDefault(0);
    match (request.path()) {
        "/health" => {
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

`sendMessage` answers a `Result`, `.Err(.Stopped)` once the actor has stopped. A
handler that cannot act on a stopped counter drops it with `withDefault` or
`ignore`, and
[`discarded-result`](../reference/lints/discarded-result.md) reports every such
decision in one list. [Tasks and actors](./concurrency.md) is the rest of the
model.

A `Server` with a `websocket` field speaks WebSockets, and the upgrade is
invisible. `onOpen` answers what the socket carries, every later hook is handed
it, and `onMessage` answers the next, so per-socket state is a value rather than
a table keyed by socket. A `Socket` is inert — one integer, sendable to an actor
that can push on it long after the request that opened it returned. The hooks are
in [the standard library](../reference/standard-library.md).

The hooks name the path they are served at, and naming it is not optional:
`WebSocket { path: "/socket", onOpen: …, onMessage: …, onClose: … }`. The match
is the request's path exactly, with no query string and no normalisation, so
`"/socket"` and `"/socket/"` are two different paths. A request to any other
path reaches `onRequest`, upgrade headers and all.

### Dial one instead

The other end of the same socket is `core/net/websocket`, and it is the same
three hooks. `connect` dials, runs them, and answers the `CloseReason` the
socket ended with.

```buri
# from "core/effect" import { ServeError, Sockets, WebSocketClient };
# from "core/net/server" import { CloseReason };
# from "core/net/websocket" import * as websocket;
# from "core/net/websocket" import { Client };

/// Subscribes once, then counts every frame the server pushes back.
fn following<C: Sockets + WebSocketClient>(ctx: C): Result<CloseReason, ServeError> {
    websocket.connect(ctx, Client {
        url: "ws://127.0.0.1:3000/socket",
        onOpen: fn(c, socket, _response) => {
            let _sent = socket.send(c, .Text("subscribe"));
            0
        },
        onMessage: fn(_c, _socket, seen, _message) => seen + 1,
        onClose: fn(_c, _socket, _seen, _reason) => (),
    })
}
```

The context grants `WebSocketClient` for the dialling and `Sockets` for the
pushing, and every platform grants both — a page or a worker can dial even
though neither can listen. An `.Err` is a socket that never opened; a socket
that opened and then ended is an `.Ok` carrying the reason.

`onOpen` is handed the `Response` that opened the socket, where the server's is
handed the `Request` that asked. That is where a negotiated subprotocol arrives,
and it is the only shape difference between the two ends.

### Reconnecting is a loop

There is no reconnect field, no backoff setting and no retry count, because
`connect` returns when the socket closes. Call it again:

```buri
# from "core/effect" import { Clock, ServeError, Sockets, WebSocketClient };
# from "core/net/server" import { CloseReason };
# from "core/net/websocket" import * as websocket;
# from "core/net/websocket" import { Client };
# from "core/time" import * as time;

/// Dials again every time the socket ends, waiting longer after each try.
fn staying<C: Clock + Sockets + WebSocketClient>(
    ctx: C,
    client: Client<C, Int>,
    waitMs: Int,
): Result<CloseReason, ServeError> {
    match (websocket.connect(ctx, client)) {
        .Err(never) => .Err(never),
        .Ok(_ended) => {
            let _slept = time.sleepMs(ctx, waitMs);
            staying(ctx, client, waitMs * 2)
        },
    }
}
```

That is an ordinary tail call, so a client that reconnects a million times costs
one stack frame. Give it a try count if you want it to stop.

### Testing one needs no network

`sockets().dialling([.Text("hi")])` is a client with a script instead of a
server, and the pushes your hooks make land in that `sockets()` double's
`sent()`. Every dial replays the script, so the loop above runs under it too.

### On a page and in a worker

`connect` follows `ui.mount`: it suspends without holding the event loop, so an
interface goes on rendering while the socket is idle and a pushed frame wakes it
like a click. What differs off the native platforms is who writes the handshake.
`LINUX` and `MACOS` write it here and check every clause of the answer;
everywhere else the engine's own `WebSocket` does, so how strictly it refuses a
bad `101` is that engine's decision, and `onOpen`'s `Response` carries the
negotiated subprotocol and extensions rather than the head the server sent.

## Stopping

`SIGTERM` and `SIGINT` do not kill a program holding a port. The platform stops
accepting, answers the requests in flight, and tells the accept loop the listener
is closed. So `serve` returns `.Ok(())` and whatever a program does after `serve`
still happens:

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
here binds `Listen`, opens a port or starts a server:

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
binary's entry point: `from "//cmd/server/main.buri" import { broadcast, hits,
route };`. That file is the whole of a binary's surface. [Testing your
code](./testing.md) is the rest.

```text
$ buri test //cmd/server
3 passed, 0 failed, 0 skipped (0.3s)
```

Two of the three authorities have a double in `core/host/testing`, and the third
does not:

| | |
|---|---|
| `sockets()` | `open()` mints a `Socket` with no network behind it, `sent()` reads back `[(Socket, Message)]`, and `isOpen(socket)` says whether it is still one this double accepts. A fresh double per call, so what one test opens is invisible to the next |
| `tasks()` | Program order by default, then `anyOrder()`, `seed(n)`, `everyOrder()` and `faults([...])` — the double whose subject is scheduling rather than state |
| `Listen` | **No double.** What a fake acceptor answers is the test's own decision, so it is a struct with an `impl Listen`, written where it is needed |

A hand-written `Sockets` could record nothing, because an effect method takes
only `self` and `self` is immutable. So the recording half has to be a handle
into runner-side state, and the deciding half does not.

## What refuses to serve

| Effect | Granted on |
|---|---|
| `Listen`, `Sockets` | `LINUX`, `MACOS` |

Under `platform: WEB` the compiler refuses this program on the line that asked
for a listener. `Tasks` is granted everywhere, so it is not one of them:

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
```

The compiler checks each entry of `outputs` against the whole graph separately,
so a binary can pass for MACOS and fail for JS.
[Compile to JavaScript](./compile-to-js.md) is that half.

## Next

- [Tasks and actors](./concurrency.md) — `parallel`, actors as values, and what
  bounds a step.
- [Effects and capabilities](./effects.md) — where a program's authority is
  written.
- [The standard library](../reference/standard-library.md) — every `Server`
  field, the WebSocket hooks, and the drain.
