## The standard library

The standard library ships with the toolchain. It is never listed in a
`dependencies`, it is available to every target, and it cannot be replaced —
there is one, and this is it. It owns two reserved module roots: `core/*`, the
deliberately small set of essentials, and `ui/*`, the reactivity vocabulary,
which is a different kind of thing and a much larger surface.

**The reference for a module is the module.** `buri docs core/list` renders it
from the source the compiler checked, so a signature on the page is the
signature that exists, and `buri docs core/list.map` renders one item of it.
`buri docs` lists [every module](../../compiler/standard_library/sources/). This
page is the map over the top of that: which modules there are, what each one
costs, and what is deliberately absent.

### The purity tiers

Every function sits in one of three tiers, and the tier is visible in the
signature rather than in a comment. This is
[`language/effects.md` §10.5](../language/effects.md), applied:

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` parameter | `xs.len()`, `date.weekday(d)`, `v.dot(o)` |
| **Deterministic** | `ctx` bounded by `Alloc` only | `xs.map(ctx, f)`, `json.stringify(ctx, v)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)`, `time.now(ctx)` |

The rule that decides it: an operation whose result size is fixed is pure, and
one whose result size depends on runtime data names `Alloc`. `len` and `fold`
are pure; `map` and `filter` are not. A `F32x4` is four numbers in a struct, so
every operation in `core/simd` is pure — the vector types are exactly the shape
the rule was drawn around.

### Values and control

[`core/option`](../../compiler/standard_library/sources/option.buri),
[`core/result`](../../compiler/standard_library/sources/result.buri),
[`core/order`](../../compiler/standard_library/sources/order.buri),
[`core/num`](../../compiler/standard_library/sources/num.buri),
[`core/bool`](../../compiler/standard_library/sources/bool.buri),
[`core/math`](../../compiler/standard_library/sources/math.buri),
[`core/bits`](../../compiler/standard_library/sources/bits.buri).

`Option`, `Result`, `Order` and the comparison and operator traits are in the
prelude, so `derive Eq for Point;` works in a module that imports nothing.

### Text

[`core/str`](../../compiler/standard_library/sources/str.buri),
[`core/char`](../../compiler/standard_library/sources/char.buri),
[`core/bytes`](../../compiler/standard_library/sources/bytes.buri),
[`core/json`](../../compiler/standard_library/sources/json.buri),
[`core/proto`](../../compiler/standard_library/sources/proto.buri).

- **`core/bytes`** — UTF-8, hex, base64, varints. Free functions rather than
  methods on `[U8]`: a method may only be declared in its type's defining
  module, and `[T]`'s is `core/list`. Decoding is strict, and validates before
  it allocates: an overlong UTF-8 encoding, a truncated sequence, or a
  surrogate is an error at a named index, not a replacement character.

- **Hexadecimal is one story across four modules, and none of it needs a table
  of digits.** `char.fromDigit(n, radix)` and `char.toDigit(radix)` are
  inverses over base 2 to base 36, `char.isHexDigit` is the predicate,
  `num.toHex(ctx, x, width)` renders a number zero-padded and lowercase — the
  64-bit two's complement, so a negative number is its bit pattern rather than
  a `-` — and `str.toRadix(text, radix)` reads any of those bases back,
  answering `.None` rather than a value the `Int` cannot hold.
  `bytes.toHex`/`bytes.fromHex` are the byte-string pair, and `toHex` walks the
  digits rather than the bytes so that rendering a megabyte is one allocation
  and not a million.

  The varints live here rather than in `core/proto`, beside hex and base64,
  because a varint is an encoding of a number as bytes, it has exactly one
  definition, and anything speaking a length-prefixed format needs the same
  one. They do 64-bit arithmetic on two 32-bit halves, so a negative `int64`
  writes the ten bytes protoc writes, and every digit of a value past 2^53
  survives on every backend.

- **`core/json`** — a `Json` tree, `parse`, and `stringify`. **An object is an
  ordered association list, not a map**, so key order round-trips, no `Hash`
  bound is needed, and `get` is O(n). Every number is a `Float`, which is what
  JSON says a number is. Nesting is capped at `MAX_DEPTH`, because parsing
  recurses and the recursion is not in tail position — without the cap a deep
  enough document is a crash rather than an error.

  **`derive ToJson` and `derive FromJson` map it to your own types**, and
  `encode` and `decode` are the two functions that use them. Both are on
  `derive`'s fixed list — no reflection and no macro is involved. The mapping
  from Buri shapes onto JSON ones is stated in the module's own source; the
  decisions with something at stake in them are that an enum is externally
  tagged, that a positional struct is an array whatever its arity, and that
  `Option<T>` is `T` or `null` — so `Option<Option<T>>` does not round-trip.

  Both traits are **derived and never written by hand**, which the compiler
  enforces. A derived encoder stands for the type's shape, so a hand-written
  one would be called where the type is encoded on its own and silently skipped
  where a type holding it is.

- **`core/proto`** — the protobuf wire format: tags, wire types, the packed
  readers, and `ProtoError`. Nothing here is written by hand either, but for a
  different reason: a `.proto` schema in a package *becomes* a module, and this
  is the part of that generated code which is the same for every schema. See
  [the proto reference](./build/proto.md) for the mapping and for
  why those codecs are generated Buri rather than a descriptor walk.

  Reading a request and writing a reply over a pipe is what `Stdin.readBytes`
  and `Stdout.writeBytes` are for: `readLine` reads the stream to its end, so a
  program using it cannot answer before the other side has finished speaking.
  Text and octets are two questions about one stream, so they are two
  operations, and a program should ask only one of them.

### Collections

[`core/list`](../../compiler/standard_library/sources/list.buri),
[`core/queue`](../../compiler/standard_library/sources/queue.buri),
[`core/map`](../../compiler/standard_library/sources/map.buri),
[`core/set`](../../compiler/standard_library/sources/set.buri),
[`core/ordmap`](../../compiler/standard_library/sources/ordmap.buri),
[`core/ordset`](../../compiler/standard_library/sources/ordset.buri),
[`core/bitset`](../../compiler/standard_library/sources/bitset.buri).

Every one of these is a value, so every "modification" returns a new one. That
is not free, and the cost is stated per module rather than implied:

| | Lookup | Insert | Note |
|---|---|---|---|
| `core/queue` | O(1) | O(1) amortized | Banker's deque: two lists, the front reversed. The reversal is what makes both ends an append. |
| `core/map`, `core/set` | O(1) expected | O(b) in buckets | Buckets of association lists. Grows and rehashes past a load factor of 4. **Iteration order is unspecified and will change.** |
| `core/ordmap`, `core/ordset` | O(log n) | O(log n) | A persistent B-tree, seven entries to a node. **Iteration is in key order**, and `range` and `prefix` are scans that cost O(log n + m) rather than filters over everything. |
| `core/bitset` | O(1) | O(n/32) | 32 bits to an `Int` word — 32 and not 64 because `Int` is signed, and a bit in position 63 would make every shift a question about sign extension. |

**Two keyed collections, and the choice between them is order.** `Map` hashes
and is faster to look one key up in; `OrdMap` compares and can answer "every key
between these two" or "every key starting with this" without visiting the rest.
A keyed range scan over a `Map` costs a sort per query, which is what
`core/ordmap` exists to avoid. Its keys need `Ord` rather than `Hash + Eq`, and
a compound key is a struct with `derive Ord` — a derived `Ord` compares fields
in declaration order, which is what a multi-column index wants.

`Queue`, `Map`, `Set`, `OrdMap`, `OrdSet` and `BitSet` provide `equals` rather
than deriving `Eq`, because a derived `Eq` would compare the *representation*:
two queues holding the same elements need not have the same front/back split,
two maps built in different orders need not have the same bucket layout, and two
ordered maps built in different orders need not have the same tree.

### Numbers and vectors

[`core/simd`](../../compiler/standard_library/sources/simd.buri) — `F32x4` and `I32x4`.

**On the JavaScript backend these are scalar and buy no speed.** There is no
SIMD reachable from a plain `.mjs` artifact. What they buy is the shape: a
kernel written lane-wise, with no loop-carried dependency, is the form a
backend with vector registers can lower directly, and the same kernel written
as a fold over a list is not, because a fold says "in this order". Do not
benchmark against a scalar loop expecting a win; there is not one today.

### Time

[`core/time`](../../compiler/standard_library/sources/time.buri) is the clock,
and reading it is an effect.
[`core/date`](../../compiler/standard_library/sources/date.buri) is the calendar,
and none of it is: what day of the week a date falls on does not depend on
anything.

`core/date` uses Hinnant's `days_from_civil`, which is exact over the whole
range of `Int` using integer arithmetic only. `Duration` is a length and
`Instant` is a point, and they are different types on purpose.

**Both of those types live in `core/time`.** `Duration` used to be the
calendar's, which meant `instant.plus(duration)` could not be written at all: a
method may only be declared in its receiver's defining module, so a length in
one module and a point in another can never meet on either of them. `core/date`
re-exports the name, so `from "core/date" import { Duration }` still resolves —
to the same type.

A `Duration` counts **nanoseconds**; an `Instant` counts milliseconds, which is
what the clock reports. `time.seconds(30)`, `millis`, `micros`, `nanos`,
`minutes` and `hours` build one; `add`, `sub`, `mul`, `negate` and `abs` combine
them. **Every one of those saturates**, because overflow is undefined behaviour
and a deadline is where a program can least afford it — the shape it replaces is
a `checkedMul`, then a `checkedSub`, then a decision taken from whichever sign
survived. `instant.hasPassed(deadline)` is that whole check, and its `Show`
prints `1.5s`, `300ms`, `750us` or `1ns` — the largest unit the length reaches,
with the exact fraction, and no `m` or `h` because a fraction of an hour is not
a decimal and a rendering that rounds is one a reader cannot compare against the
value.

**There is no timezone database, and there will not be one.** tzdata is
megabytes that change several times a year, and this toolchain has no
dependencies and ships no data files. `Zoned` carries a fixed offset in
minutes, which covers UTC, a stored offset, and arithmetic within one offset.
It does not cover `America/New_York`, and it does not pretend to.

### Cryptography

[`core/crypto`](../../compiler/standard_library/sources/crypto.buri) — SHA-256,
HMAC-SHA-256, and a constant-time comparison. Written in Buri rather than handed
to the platform, because a dependency tree is a second thing to audit. It is
checked against the NIST vectors, as is the independent SHA-256 the build cache
uses, in two languages neither of which can compile the other.

Deliberately absent, and not by oversight:

- **No ciphers.** Shipping a block function without a key schedule, a mode, a
  nonce discipline, and an authentication tag is how people end up with ECB.
- **No public-key anything.**
- **No cryptographically secure randomness.** `core/random` is seeded and
  reproducible on purpose — a hermetic test needs the same numbers every run.
  A CSPRNG is a different thing, and it would need a new `Entropy` effect in
  `core/effect` with a host implementation behind it. That is a decision to take
  deliberately, not a function to add quietly beside these.

`sha256` is **not a password hash**. It is fast, which is the wrong property.

### User interfaces

[`ui/effect`](../../compiler/standard_library/sources/ui_effect.buri),
[`ui/signal`](../../compiler/standard_library/sources/ui_signal.buri),
[`ui/prop`](../../compiler/standard_library/sources/ui_prop.buri),
[`ui/node`](../../compiler/standard_library/sources/ui_node.buri),
[`ui/style`](../../compiler/standard_library/sources/ui_style.buri),
[`ui/theme`](../../compiler/standard_library/sources/ui_theme.buri) and
[`ui/testing`](../../compiler/standard_library/sources/ui_testing.buri) are the
second reserved root. They have a page of their own:
[user interfaces](../guides/user-interfaces.md).

### The platform

[`core/effect`](../../compiler/standard_library/sources/effect.buri) declares the
effects; [`core/host`](../../compiler/standard_library/sources/host.buri)
implements them and may be imported only by the module that exports `main`.
[`core/alloc`](../../compiler/standard_library/sources/alloc.buri),
[`core/io`](../../compiler/standard_library/sources/io.buri),
[`core/fs`](../../compiler/standard_library/sources/fs.buri),
[`core/env`](../../compiler/standard_library/sources/env.buri),
[`core/time`](../../compiler/standard_library/sources/time.buri),
[`core/random`](../../compiler/standard_library/sources/random.buri),
[`core/net/http`](../../compiler/standard_library/sources/http.buri),
[`core/net/server`](../../compiler/standard_library/sources/server.buri),
[`core/proc`](../../compiler/standard_library/sources/proc.buri),
[`core/tasks`](../../compiler/standard_library/sources/tasks.buri) and
[`core/actor`](../../compiler/standard_library/sources/actor.buri) are the interfaces
those effects are used through — and they are the *only* way through: an effect
is performed by handing the context to a function, never by calling a method on
it (SPEC 10.2), so `io.println(ctx, text)` is how a program prints and
`ctx.println(text)` is refused.
[`core/testing/assert`](../../compiler/standard_library/sources/assert.buri) and
[`core/host/testing`](../../compiler/standard_library/sources/host_testing.buri)
are importable only from a test source.

[Build a web server](../guides/web-server.md) walks the four of them end to end,
and [tasks and actors](../guides/concurrency.md) is the concurrency model
underneath; what follows is the map.

`core/proc` is the thinnest of them: `proc.exit(ctx, code)` is `Proc`'s one
operation. `core/net/server` is the other half of `core/net/http` — a program
that *is* a server rather than one that talks to one, which is a second
authority rather than a second spelling. A `Server<C, S>` is the whole
configuration: a `port`, an `onRequest` handler taking the caller's own context,
and `Option` knobs for the address, the protocols, a certificate, a request
limit, an idle timeout, a shutdown deadline, the WebSocket hooks and a socket
buffer, each of which means "the runtime chooses" when it is left out of the
literal. `S` is what one socket carries and it is a type nothing constrains on a
server with no hooks, which the checker settles as `()` — so a program that does
not do WebSockets never spells it. `serve` binds and answers until the
listener closes;
`bind` and `run` are the same thing in two halves, for a program that wants the
port number before it starts answering; `errorText` turns a `ServeError` into a
line. It speaks HTTP/1.1 and, over TLS, HTTP/2. A `tls: .Some(Tls { certificate,
key })` names two PEM *files* — read once, when the port is opened, so a
certificate that is missing or does not match its key stops the program starting
— and turns the server into an HTTPS one without changing a handler, which never
learns which transport its request arrived on. HTTP/2 comes with TLS and only
with TLS: it is chosen inside the handshake by ALPN, so a `Server` naming
`.Http2` without a certificate is refused at the bind rather than quietly served
in HTTP/1.1, and a `Server` with a certificate and no `protocols` offers
HTTP/1.1 because `.None` is the absence of a choice. One request per HTTP/1.1
connection; several share an HTTP/2 one, which is what multiplexing is. It
answers as many at once as the acceptor said it would host — `run` puts each
handler on a task of its own, which is why `serve` needs `Tasks` and `Alloc`
beside `Listen`. Only
`LINUX` and `MACOS` grant `Listen`, because a page is served rather than
serving; `WEB` grants no `Tasks` either, so a server on a page is refused
twice.

**A `Server` with a `websocket` speaks WebSockets, and the upgrade is
invisible.** With hooks present a client that asks for a socket gets one and
`onOpen` runs; without them the same request reaches `onRequest` like any other,
and there is no branch in a handler either way. `onOpen` answers the socket's
first state, every later hook is handed the current one, and `onMessage` answers
the next — so per-socket state is a value rather than a table keyed by socket,
and the counter-per-socket example in `buri docs core/net/server` is an actor's
address. A `Socket` is inert: one integer, comparable, and sendable to an actor,
which can then push on it long after the request that opened it returned, since
`socket.send` and `socket.close` need `C: Sockets` and nothing else. `send`
never waits — it hands the message to the socket's outbound buffer, and a buffer
that fills closes the socket with `.Overflow` and runs `onClose`; `socketBuffer`
is how deep it is. A close is a `CloseReason` and never a wire code, in both
directions, and ping and pong are the platform's, so `onMessage` sees `.Text`
and `.Binary` and nothing else. What a socket costs is a worker: its whole life
runs on the one that accepted it, which is what makes the hooks on a socket run
in order by construction, and it means a server holding `listener.handlers`
sockets has none left to accept with. A socket open when a shutdown begins is
closed with `.GoingAway`, so a drain does not wait out a client that is doing
nothing wrong, and `onClose` still runs.

**A server stops gracefully.** `SIGTERM` and `SIGINT` do not kill a program that
is holding a port: the platform stops accepting connections, lets the requests
already in flight be answered, and then tells the accept loop the listener is
closed — so `serve` returns `.Ok(())`, `main` falls off its end, and whatever a
program does after `serve` still happens. `drainMillis` bounds how long the
middle step may take, and a second signal is the operating system's own, so
`Ctrl-C` twice stops a process that will not drain. A program with no listener
open is not affected at all: the signals are the platform's only while a port
is.

`core/tasks` is one function. `parallel(ctx, items, f)` runs `f` over every item
and answers the results **in the items' order**, whatever order the work
finished in, handing each call the item's own index. Every task has finished
before it returns, so nothing outlives the context that granted it:

```buri
from "core/effect" import { Alloc, Tasks };
from "core/tasks" import * as tasks;

fn squares<C: Alloc + Tasks>(ctx: C, ns: [Int]): [Int] {
    tasks.parallel(ctx, ns, fn(c, i, n) => n * n)
}
```

The `c` a task is handed is the **caller's whole context** — every effect `ctx`
carried, so a task can do anything its caller could and nothing it could not.
It arrives as a parameter because a lambda may not capture a context
(Section 10.6).

How much actually runs at once is the platform's business and not the
signature's. JavaScript starts the tasks together and awaits them together; a
native `--release` build gives each task a carrier of its own, so two that wait
overlap; `buri run` runs them in index order on one carrier, because a program
its backend builds has a single Buri stack to hold their frames in. All three
answer the same list, which is the point of fixing the order. Two tasks that
*compute* do not yet overlap on either native backend: `parallel` buys
overlapped waiting rather than more processors.

`core/actor` is the other half of concurrency: state that outlives one call,
behind a mailbox. An actor is a *value* — an initial state and a
`step: fn(C, S, M) => S` — and `start` gives it a mailbox and answers an
`Address`. The enum is the protocol: a variant carrying no `Reply` is a `send`,
a variant carrying a `Reply<R>` is an `ask` that yields an `R`, and `stop`
closes the mailbox, discards what is left and runs `onStop` once with the final
state. Everything after that is `.Err(.Stopped)`.

```buri
from "core/actor" import { Actor, Reply };

enum CounterMessage {
    Increment,
    Get(Reply<Int>),
}

fn counter<C>(initial: Int): Actor<C, Int, CounterMessage> {
    Actor { state: initial, step: fn(c, count, message) => count + 1 }
}
```

It needs no test double, and that is a property of the shape rather than an
omission: `step` is an ordinary function in an ordinary field, so testing an
actor is calling it. The mailbox is bounded — `mailbox: .Some(1)`, or the
module's own `MAILBOX` — and a `send` that fills it runs the actor down rather
than letting the queue grow. **The actor steps on the task that drives it**:
`ask` runs the mailbox down before it reads its reply, and `stop` before it
runs `onStop`. That is a scheduling decision and not a semantic one — the
answers are the same either way, exactly as `parallel`'s two arms answer the
same list — but it means an actor is not yet a way to get work done in the
background.

`core/net/http` is where `Request` and `Response` are documented — the two types
`Net.fetch` speaks in, re-exported from `core/effect` where the effect's own
signature names them. A message is built by a free function and then by
chaining:

```buri
from "core/effect" import { Alloc, Net };
from "core/net/http" import * as http;

fn ping<C: Alloc + Net>(ctx: C): Str {
    match (http.send(ctx, http.request(.Get, "http://example.com/ping"))) {
        .Ok(reply) => http.bodyText(ctx, reply.body).withDefault("not text"),
        .Err(e) => http.errorText(e),
    }
}
```

The `with*` methods — `withHeader`, `withBody`, `withStatus`, `withMethod` —
each answer a *new* message, so a request is assembled by chaining and never by
mutation. There are no associated functions in the language (a function inside
an `impl` block takes `self`), so the constructors are free functions:
`http.request`, `http.textRequest`, `http.status`, `http.ok`, `http.text`,
`http.json`.

`core/host/testing` is `core/host`'s surface for a test: the same names —
`alloc`, `stdout`, `stderr`, `stdin`, `fs`, `net`, `clock`, `rand`, `env`,
`proc`, `sockets` — **called** rather than referred to, so each one is a fresh double, and
configured by a method that answers a new one (`clock().at(1000)`,
`rand().seed(7)`, `env().variables([...]).arguments([...])`,
`fs().files([...]).readOnly()`). `net()` **refuses** every request until
`net().respond(fn(request) => ...)` says what to answer, and that responder is a
pure function of the `Request` — SPEC 10.6 keeps it from capturing a context, so
a response that needs one is built before it and captured. `fs()`, `net()` and
`stdin()` each keep a log of what they were asked: `calls()` answers it in the
order the calls completed, and a test writes down what it expects with the
constructor of the same name (`readFile(path)`, `writeFile(path, body)`,
`fetch(request)`, `readBytes(n)`, one per method). Those same constructors say
what **breaks**: `fs().faults([readFile(p).fails(.PermissionDenied)])` fails
every matching call and `failsOnCall(n, e)` the `n`th, so success comes from the
fixture and failure comes from the plan — and a fault whose call never happens
fails the test. `tasks()` is the one double whose subject is not state but
**scheduling**: `Tasks.parallel` promises its results in the items' order and
nothing about the order the work runs in, so `tasks()` runs the tasks in program
order, `tasks().anyOrder()` in the one order its own content seeds, and
`tasks().everyOrder()` runs
the whole `test` body once per completion order. A seed is the order's own
number, so a failure names a line that replays it. `sockets()` is the double for
the writing half of a WebSocket: `sockets().open()` mints a `Socket` with no
network behind it, `sent()` reads back `[(Socket, Message)]` and `isOpen(s)`
says whether a socket is still one this double will take a message for — so a
broadcast room, which is `Sockets` and nothing else, is tested with no listener,
no port and no client. See
[testing](./build/testing.md).

### Allocators

[`core/alloc`](../../compiler/standard_library/sources/alloc.buri) —
`GeneralPurpose`, `Arena`, `FixedBuffer`. Three implementations of `Alloc`,
importable anywhere, because `Alloc` is the one effect whose
implementation carries no authority: a `Region` is a number, so a library that
builds its own allocator has been granted nothing.

- **`GeneralPurpose`** — unbounded, counts. `gp.stats()` answers
  `Stats { allocations, bytes }`.
- **`FixedBuffer(n)`** — a byte budget, and charging past it **aborts**. That
  is forced rather than chosen: `allocate` answers `Region` and not
  `Result<Region, _>`, so there is no value to report a failure with, and
  [`language/expressions.md` §6.10](../language/expressions.md) says that is
  what an abort is for. The message carries both numbers.
- **`Arena`** — a separate counter, and nothing more than a counter. It does
  not free in bulk, and it says so.

`core/alloc` also has the **scope**:

```buri
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Fs };
from "core/fs" import * as fs;

fn inAScope<C: Alloc + Fs>(ctx: C, path: Str): Bool {
    alloc.scoped(ctx, fn(c) => fs.exists(c, path))
}
```

`scoped(ctx, body)` runs `body` with a `Scoped<C>` — an attenuating wrapper
that forwards every effect `ctx` grants and replaces one. Its `Alloc` is the
scope's own arena, so a charge inside reserves from that arena and the caller's
allocator's totals do not move; when `body` returns, the arena's pages go back
to the platform. Everything else is unchanged: the body prints on the same
stdout, reads the same files and fans out onto the same tasks.

It holds the **values** too. A `[Str]` a scope builds is in the arena's own
pages, and they go back with the rest when `body` returns — so a scope is a
lifetime and not only a budget. The one value that leaves is `body`'s answer,
and it is deep-copied onto the caller's allocator first, at every depth: a
nested list, an enum's payload, a closure's captured environment. You never
write the copy and cannot observe it except as a cost.

Two consequences worth knowing. **Answer only what you need** — the copy is
proportional to what leaves, so a scope that answers a whole parsed document
copies a whole parsed document. And **a task started inside a scope allocates
outside it**, on the ordinary heap: the arena belongs to the carrier that
entered the scope, and a step of a `Tasks.parallel` runs somewhere else.

What an allocator is told about is narrower than what the cost model defines,
identically on both backends: **every `allocate(ctx, n)`, and nothing else.**
The charge for an operation is *defined* rather than measured — a `Str` of *n*
UTF-8 bytes charges `16 + n`, a `[T]` of *n* charges `16 + n * stride(T)`, a
view charges nothing — and the list and string rows are charged by definition
and reported to no allocator. The model is written down beside `Alloc` in
`core/effect`, where a reader of the effect meets it.

### What is deliberately not here

- **Struct-of-arrays / `MultiArrayList`.** Not typeable today: exposing "column
  *i* of `T`, at `T`'s *i*-th field type" needs dependent or row types, and
  [`language/types.md` §5.5](../language/types.md) has no records. Write the two-field
  struct yourself; on the JavaScript backend that is all a library would do.
- **Bulk reclamation outside a scope.** `scoped` frees in bulk because it knows
  when it is over and copies its answer out. `Arena` — the type you carry
  around — has no boundary to copy at, so it stays a counter: it answers "how
  much did parsing charge?" and reclaims nothing a `GeneralPurpose` would not
  have reclaimed anyway.
- **Automatic accounting of the list and string rows.** Stated above: the cost
  model defines them and no allocator is told about them.

Why each of those is where it is, and what would have to change, is in
[`design/STANDARD-LIBRARY.md`](../../../../design/STANDARD-LIBRARY.md) — that is a
contributor's document, not a user's.
