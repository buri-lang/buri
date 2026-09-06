# The standard library

The standard library ships with the toolchain. You never list it in a
`dependencies`, every target can use it, and nothing replaces it. There is one,
and this is it. It owns two reserved module roots. `core/*` is a deliberately
small set of essentials. `ui/*` is the reactivity vocabulary, a different kind
of thing and a much larger surface.

**The reference for a module is the module.** `buri docs core/list` renders it
from the source the compiler checked, so a signature on the page is a signature
that exists. `buri docs core/list.map` renders one item of it. `buri docs` lists
[every module](../../compiler/standard_library/sources/). This page maps over
the top of that: which modules there are, what each one costs, and what is
deliberately absent.

## The purity tiers

Every function sits in one of three tiers. The signature shows which one, so no
comment has to. This is [`language/effects.md` §10.5](../language/effects.md),
applied:

| Tier | Shape | Example |
|---|---|---|
| **Pure** | no `ctx` parameter | `xs.len()`, `date.weekday(d)`, `v.dot(o)` |
| **Deterministic** | `ctx` bounded by `Alloc` only | `xs.map(ctx, f)`, `json.stringify(ctx, v)` |
| **Effectful** | `ctx` bounded by anything else | `fs.readText(ctx, p)`, `time.now(ctx)` |

One rule decides the tier. An operation with a fixed result size is pure. An
operation whose result size depends on runtime data names `Alloc`. So `len` and
`fold` are pure, and `map` and `filter` are not. A `F32x4` is four numbers in a
struct, so every operation in `core/simd` is pure. The vector types are exactly
the shape the rule was drawn around.

## Values and control

[`core/option`](../../compiler/standard_library/sources/option.buri),
[`core/result`](../../compiler/standard_library/sources/result.buri),
[`core/order`](../../compiler/standard_library/sources/order.buri),
[`core/num`](../../compiler/standard_library/sources/num.buri),
[`core/bool`](../../compiler/standard_library/sources/bool.buri),
[`core/math`](../../compiler/standard_library/sources/math.buri),
[`core/bits`](../../compiler/standard_library/sources/bits.buri).

`Option`, `Result`, `Order` and the comparison and operator traits are in the
prelude, so `derive Eq for Point;` works in a module that imports nothing.

**A comparator is a value, and `core/order` builds one.** `order.by` takes the
key. `order.chain` takes the tie-breaks in priority order. `order.reverseIf`
takes the direction from the data. So a sort key with three columns and a `DESC`
is a value rather than a `match` written out. `order.int`, `float`, `str`,
`bool` and `char` are the primitives underneath them. Sort `Float` data with
`order.totalFloat`: `order.float` follows IEEE, and IEEE leaves a `NaN`
unordered, so it answers `.Equal` for a pair it could not order.

## Text

[`core/str`](../../compiler/standard_library/sources/str.buri),
[`core/char`](../../compiler/standard_library/sources/char.buri),
[`core/bytes`](../../compiler/standard_library/sources/bytes.buri),
[`core/json`](../../compiler/standard_library/sources/json.buri),
[`core/proto`](../../compiler/standard_library/sources/proto.buri).

- **`core/str`** — a `Str` measures in Unicode scalar values everywhere. `len`
  counts them, `charAt` and `slice` index by them, and `compare` orders by them.
  Read that last one before you rely on it. Two orders are plausible, and they
  disagree above the basic multilingual plane. `compare` uses scalar order,
  which is byte-for-byte UTF-8 order for a valid string, as in Rust, Go and
  Python. It is *not* the UTF-16 code-unit order a JavaScript `<` gives, on
  either backend. `<`, `[Str].sort`, `core/order`'s `str` and an
  `OrdMap<Str, _>`'s key order all use that one comparison.

- **`core/bytes`** — UTF-8, hex, base64, varints. These are free functions
  rather than methods on `[U8]`, because you may only declare a method in its
  type's defining module, and `[T]`'s is `core/list`. Decoding is strict and
  validates before it allocates: an overlong UTF-8 encoding, a truncated
  sequence, or a surrogate comes back as an error at a named index, never as a
  replacement character.

- **Hexadecimal is one story across four modules, and none of it needs a table
  of digits.** `char.fromDigit(n, radix)` and `char.toDigit(radix)` invert each
  other over base 2 to base 36, and `char.isHexDigit` is the predicate.
  `num.toHex(ctx, x, width)` renders a number zero-padded and lowercase in
  64-bit two's complement, so a negative number comes out as its bit pattern
  rather than a `-`. `str.toRadix(text, radix)` reads any of those bases back,
  answering `.None` rather than a value the `Int` cannot hold.
  `bytes.toHex`/`bytes.fromHex` are the byte-string pair. `toHex` walks the
  digits rather than the bytes, so rendering a megabyte costs one allocation
  and not a million.

  The varints live here beside hex and base64 rather than in `core/proto`. A
  varint encodes a number as bytes, it has exactly one definition, and anything
  speaking a length-prefixed format needs that same one. They do 64-bit
  arithmetic on two 32-bit halves, so a negative `int64` writes the ten bytes
  protoc writes, and every digit of a value past 2^53 survives on every
  backend.

  A **`Reader`** gives a name to the index that `readVarint(b, at)` threads.
  `takeByte`, `takeVarint`, `takeSlice` and `takeFramed` each answer the value
  and the *next* reader, rather than moving this one. A reader is a value like
  everything else, so a decoder that looks ahead and changes its mind still
  holds the reader it started from. The methods that only move the cursor are
  pure. The two that answer a `[U8]` name `Alloc`, because a Buri list is a
  value and not a view, so slicing one copies. For the same reason there is
  **no `Builder`**: nothing appends to a value in place, and `[[U8]].flatten`
  is what building looks like here.

  `fromU64Be` and its seven relatives cover both ends of both widths, in both
  directions. Writing one is *pure*: an array literal of a fixed size allocates
  nothing a context has to grant. Reading answers an `Option`, because a number
  assembled out of octets that were not there is a wrong answer wearing the
  shape of a right one.

- **`core/json`** — a `Json` tree, `parse`, and `stringify`. **An object is an
  ordered association list, not a map**, so key order round-trips, nothing needs
  a `Hash` bound, and `get` costs O(n). Every number is a `Float`, which is what
  JSON says a number is. `MAX_DEPTH` caps nesting, because parsing recurses and
  the recursion is not in tail position. Without the cap, a deep enough document
  crashes rather than erroring.

  **`derive ToJson` and `derive FromJson` map it onto your own types**, and
  `encode` and `decode` are the two functions that use them. Both sit on
  `derive`'s fixed list, so no reflection and no macro is involved. The module's
  own source states how Buri shapes map onto JSON ones. Three of those decisions
  have something at stake: an enum is externally tagged, a positional struct is
  an array whatever its arity, and `Option<T>` is `T` or `null`. That last one
  means `Option<Option<T>>` does not round-trip.

  The compiler enforces that you **derive both traits and never write them by
  hand**. A derived encoder stands for the type's shape. A hand-written one
  would run where something encodes the type on its own, and be skipped
  silently where something encodes a type holding it.

- **`core/proto`** — the protobuf wire format: tags, wire types, the packed
  readers, and `ProtoError`. You write none of this by hand either, for a
  different reason. A `.proto` schema in a package *becomes* a module, and this
  module is the part of that generated code that stays the same for every
  schema. See [the proto reference](./build/proto.md) for the mapping, and for
  why those codecs are generated Buri rather than a descriptor walk.

  `Stdin.readBytes` and `Stdout.writeBytes` are for reading a request and
  writing a reply over a pipe. `readLine` reads the stream to its end, so a
  program using it cannot answer before the other side has finished speaking.
  Text and octets are two questions about one stream, so they are two
  operations, and a program should ask only one of them.

## Collections

[`core/list`](../../compiler/standard_library/sources/list.buri),
[`core/queue`](../../compiler/standard_library/sources/queue.buri),
[`core/map`](../../compiler/standard_library/sources/map.buri),
[`core/set`](../../compiler/standard_library/sources/set.buri),
[`core/ordmap`](../../compiler/standard_library/sources/ordmap.buri),
[`core/ordset`](../../compiler/standard_library/sources/ordset.buri),
[`core/bitset`](../../compiler/standard_library/sources/bitset.buri).

Every one of these is a value, so every "modification" answers a new one. That
costs something, and each module states its cost rather than leaving you to
guess:

| | Lookup | Insert | Note |
|---|---|---|---|
| `core/queue` | O(1) | O(1) amortized | Banker's deque: two lists, the front reversed. The reversal makes both ends an append. |
| `core/map`, `core/set` | O(1) expected | O(b) in buckets | Buckets of association lists. Grows and rehashes past a load factor of 4. **Iteration order is unspecified and will change.** |
| `core/ordmap`, `core/ordset` | O(log n) | O(log n) | A persistent B-tree, seven entries to a node. **Iteration runs in key order.** `range` and `prefix` scan at O(log n + m) rather than filtering over everything. |
| `core/bitset` | O(1) | O(n/32) | 32 bits to an `Int` word. 32 and not 64 because `Int` is signed, and a bit in position 63 would make every shift a question about sign extension. |

**Two keyed collections, and order is what you choose between.** `Map` hashes,
and looks one key up faster. `OrdMap` compares, and answers "every key between
these two" or "every key starting with this" without visiting the rest. A keyed
range scan over a `Map` costs a sort per query, which is what `core/ordmap`
avoids. Its keys need `Ord` rather than `Hash + Eq`. A compound key is a struct
with `derive Ord`, and a derived `Ord` compares fields in declaration order,
which is what a multi-column index wants.

**A fallible step is a traversal, not a fold.** `xs.mapResult(ctx, f)` and
`mapOption` map every element, or stop at the first that fails. `filterMap` maps
and filters in one pass. Without these three you write a `foldResult` at every
call site. Each has a `*Ctx` form that hands the step the context, because a
validation usually allocates as it goes and a lambda may not capture a context.
Beside them in `core/list`: `removeAt`, `windows`, `generate`, `uniqueBy`,
`isSortedBy`, `maxBy`/`minBy` and `compareBy`. The last three are the comparator
forms of operations `core/list` already had for an `Ord`. `uniqueBy` keeps the
first of each equal class, so it costs O(n²) in comparisons. Where the order may
change, `sortBy` and a walk is the O(n log n) answer.

**Grouping answers a map, so it lives with the map.** `map.groupBy(ctx, xs,
key)` and `ordmap.groupBy` collect the elements under each key, and `indexBy`
keeps one element per key. They are free functions because `core/list` sits at
the bottom of the dependency order and cannot name a map. `OrdMap.alter` is
insert, replace and remove in one call, which is what a counter needs, or the
empty-inner pruning of a map of maps. `OrdMap.mapValues` puts every value
through a function without touching the keys, rebuilding the tree as it goes.
That costs O(n log n), rather than the O(n) a copy node for node would cost.

`Queue`, `Map`, `Set`, `OrdMap`, `OrdSet` and `BitSet` provide `equals` rather
than deriving `Eq`, because a derived `Eq` would compare the *representation*.
Two queues holding the same elements need not share a front/back split. Two maps
built in different orders need not share a bucket layout. Two ordered maps built
in different orders need not share a tree.

## Numbers and vectors

[`core/simd`](../../compiler/standard_library/sources/simd.buri) — `F32x4` and `I32x4`.

**On the JavaScript backend these are scalar and buy no speed.** A plain `.mjs`
artifact reaches no SIMD. What they buy is the shape. A kernel written
lane-wise, with no loop-carried dependency, is the form a backend with vector
registers can lower directly. The same kernel written as a fold over a list is
not, because a fold says "in this order". Do not benchmark against a scalar loop
expecting a win. There is not one today.

## Time

[`core/time`](../../compiler/standard_library/sources/time.buri) is the clock,
and reading it performs an effect.
[`core/date`](../../compiler/standard_library/sources/date.buri) is the calendar,
and none of it performs one: what day of the week a date falls on does not
depend on anything.

`core/date` uses Hinnant's `days_from_civil`. It does integer arithmetic only
and stays exact over the whole range of `Int`. `Duration` is a length and
`Instant` is a point, and they are different types on purpose.

**Both of those types live in `core/time`.** `Duration` used to belong to the
calendar, and nobody could write `instant.plus(duration)` at all. You may only
declare a method in its receiver's defining module, so a length in one module
and a point in another never meet on either of them. `core/date` re-exports the
name, so `from "core/date" import { Duration }` still resolves, to the same
type.

A `Duration` counts **nanoseconds**. An `Instant` counts milliseconds, which is
what the clock reports. `time.seconds(30)`, `millis`, `micros`, `nanos`,
`minutes` and `hours` build one, and `add`, `sub`, `mul`, `negate` and `abs`
combine them. **Every one of those saturates.** Overflow is undefined behaviour,
and a deadline is where a program can least afford it. Saturating replaces a
`checkedMul`, then a `checkedSub`, then a decision taken from whichever sign
survived. `instant.hasPassed(deadline)` is that whole check. Its `Show` prints
`1.5s`, `300ms`, `750us` or `1ns`: the largest unit the length reaches, with the
exact fraction. There is no `m` or `h`, because a fraction of an hour is not a
decimal, and a reader cannot compare a rendering that rounds against the value.

**There is no timezone database, and there will not be one.** tzdata runs to
megabytes and changes several times a year, and this toolchain has no
dependencies and ships no data files. `Zoned` carries a fixed offset in
minutes, which covers UTC, a stored offset, and arithmetic within one offset.
It does not cover `America/New_York`, and it does not pretend to.

## Randomness

[`core/random`](../../compiler/standard_library/sources/random.buri) has two
doors. The split follows one principle: **an RNG either takes a seed or takes a
context**.

`int`, `float` and `bytes` take a context and perform the `Rand` effect. `Gen`
takes a seed and performs nothing. `random.seeded(7)` is an ordinary value,
every method answers `(value, Gen)`, and the same seed gives the same sequence
on every backend and in every process. `Gen` is splitmix64, published in the
module rather than hidden behind an effect. `split()` answers two streams, where
a program would otherwise invent salt constants by hand.

A generator that is a value is what a deterministic simulator needs and could
not otherwise have. A simulation replays a failure from a seed, so it cannot
take its generator from whoever called it. `random.gen(ctx)` bridges the two:
draw a seed from the platform once, then stay pure.

`Gen.nextInt` rejection-samples, so it has **no modulo bias**. The bounded draw
is where a hand-written generator keeps going wrong.

Neither door is a secret. Both are uniform and both are predictable. For `Gen`,
a single draw predicts the rest, which is what publishing the algorithm means.
For octets nobody can guess, see [`core/crypto`](#cryptography) below.

## Checksums

[`core/hash`](../../compiler/standard_library/sources/hash.buri) — `fnv1a32`,
`fnv1a64`, `crc32c` and `siphash24`, pure over `[U8]`.

**This is not [`core/crypto`](#cryptography), and that is the whole reason it is
a module of its own.** Nothing here is a digest. Given a target value, producing
a message that hashes to it is arithmetic rather than work. Use these where a
digest is the wrong size: a flipped bit in a log record, a bucket index, a
fingerprint you can compare two runs of a simulator on. A storage format puts
four octets of guard on a record on purpose. Before this module existed, that
decision cost every repository that made it a hand-written FNV-1a.

Write `crc32c` into a record, because a storage format's readers already expect
it. `siphash24` is the only one that takes a key, and the key is the point. Put
an unkeyed hash behind a map whose keys arrive from outside, and someone can
hand you a thousand keys that all land in one bucket.

Each one is written in Buri and pinned to the vectors its publisher wrote down:
Noll's for FNV-1a, the CRC-32C check value and RFC 3720 B.4's iSCSI cases, and
the SipHash-2-4 reference table. So the answer is the same on both backends, and
it is the answer another implementation gives. That also makes the package a
hard test of the language: U32 and U64 wrapping arithmetic, both shifts, and the
same numbers where a `U64` is a machine word and where it is a `BigInt`.

## Cryptography

[`core/crypto`](../../compiler/standard_library/sources/crypto.buri) — SHA-256,
HMAC-SHA-256, a constant-time comparison, and the platform's cryptographic
randomness.

The hashes are written in Buri rather than handed to the platform, because a
dependency tree is a second thing to audit. The NIST vectors check them, and
check the independent SHA-256 the build cache uses, in two languages neither of
which can compile the other.

`randomBytes` and `token` are the half *not* written here. They perform the
`Entropy` effect, and the operating system supplies the octets: `getrandom(2)`
and `getentropy(2)` under a native binary, `crypto.getRandomValues` under a
JavaScript one. A platform supplies a CSPRNG, and nobody reimplements the
algorithm. It performs an effect rather than being a plain function, and that is
the point of the whole arrangement: a program has to be able to *ask* for
unguessability, and to be refused where it cannot be had.

**`randomBytes` is in `core/crypto` and not in `core/random`, deliberately.**
`core/random` is seeded and reproducible on purpose, because a hermetic test
needs the same numbers every run, and its own `bytes` keeps that promise. You
cannot tell the two sets of octets apart by inspection. They differ only in
whether an observer can predict the next one, so a program says which it meant
by the module it imports. `token(ctx, 32)` is the spelling for a session's
resume token: 32 octets of entropy, written as lowercase hex.

Every platform grants `Entropy`. What can be missing is the *toolchain*: a
runtime archive built without its `crypto` feature refuses `randomBytes` by
name, before code generation, rather than answering from a generator that is
merely uniform. See
[cryptography-not-available](./errors/cryptography-not-available.md).

Deliberately absent, and not by oversight:

- **No ciphers, yet.** Ship a block function without a key schedule, a mode, a
  nonce discipline and an authentication tag, and people end up with ECB. It
  would take the shape of one authenticated construction: an AEAD, with
  `Entropy` minting the nonce rather than the caller. That is a slice of work on
  its own. It needs a second host effect on both backends. `crypto.subtle` is
  undefined on a page that is not a secure context, while
  `crypto.getRandomValues` stays defined there. And every Buri function that
  could reach it becomes `async` in the emitted JavaScript, because
  `crypto.subtle` is promise-shaped. Each of those has an answer. None has a
  quiet one.
- **No public-key anything.**
- **No key derivation and no password hashing.**

`sha256` is **not a password hash**. It is fast, which is the wrong property.

It is also the wrong size for a flipped-bit guard: thirty-two octets of frame on
a log record where four would do. [`core/hash`](#checksums) covers that case.
The two modules stay separate so a program says which it meant by which one it
imports.

## User interfaces

[`ui/effect`](../../compiler/standard_library/sources/ui_effect.buri),
[`ui/signal`](../../compiler/standard_library/sources/ui_signal.buri),
[`ui/prop`](../../compiler/standard_library/sources/ui_prop.buri),
[`ui/node`](../../compiler/standard_library/sources/ui_node.buri),
[`ui/style`](../../compiler/standard_library/sources/ui_style.buri),
[`ui/theme`](../../compiler/standard_library/sources/ui_theme.buri) and
[`ui/testing`](../../compiler/standard_library/sources/ui_testing.buri) are the
second reserved root. They have a page of their own:
[user interfaces](../guides/user-interfaces.md).

## The platform

[`core/effect`](../../compiler/standard_library/sources/effect.buri) declares
most of the effects, and
[`core/fs`](../../compiler/standard_library/sources/fs.buri) declares the
filesystem's two.
[`core/host`](../../compiler/standard_library/sources/host.buri)
implements them all, and only the module that exports `main` may import it.
[`core/alloc`](../../compiler/standard_library/sources/alloc.buri),
[`core/io`](../../compiler/standard_library/sources/io.buri),
[`core/fs`](../../compiler/standard_library/sources/fs.buri),
[`core/path`](../../compiler/standard_library/sources/path.buri),
[`core/env`](../../compiler/standard_library/sources/env.buri),
[`core/cli`](../../compiler/standard_library/sources/cli.buri),
[`core/time`](../../compiler/standard_library/sources/time.buri),
[`core/random`](../../compiler/standard_library/sources/random.buri),
[`core/crypto`](../../compiler/standard_library/sources/crypto.buri),
[`core/net/http`](../../compiler/standard_library/sources/http.buri),
[`core/net/server`](../../compiler/standard_library/sources/server.buri),
[`core/proc`](../../compiler/standard_library/sources/proc.buri),
[`core/tasks`](../../compiler/standard_library/sources/tasks.buri) and
[`core/actor`](../../compiler/standard_library/sources/actor.buri) are the interfaces
you use those effects through, and the *only* way through. A program performs an
effect by handing the context to a function, never by calling a method on it
(SPEC 10.2). So `io.println(ctx, text)` is how a program prints, and the
compiler refuses `ctx.println(text)`.
Only a test source may import
[`core/testing/assert`](../../compiler/standard_library/sources/assert.buri) and
[`core/host/testing`](../../compiler/standard_library/sources/host_testing.buri).
`assert` is deliberately wide — `eq`,
`notEq`, `isTrue`, `isFalse`, `contains`, `isEmpty`, `notEmpty`, `len`, `gt`,
`ge`, `lt`, `le`, `approxEq`, and the unwrapping `ok`, `err`, `some`, `none` —
because the report is the point. Each one names the two values it compared,
where `assert.isTrue(xs.contains(x))` can only say "expected true, got false".
There is no `assert.fail`. It answered `()` rather than a bottom type, so a
match arm using it could not produce a value, and the test had to fabricate one.

[Build a web server](../guides/web-server.md) walks the four of them end to end.
[Tasks and actors](../guides/concurrency.md) is the concurrency model
underneath. What follows is the map.

`core/proc` is the thinnest of them. `proc.exit(ctx, code)` is `Proc`'s one
operation.

`core/env` and `core/cli` are the two halves of a command line. `env.args(ctx)`
is the raw `[Str]`. Both hosts drop the program's own name, so there is no
`argv[0]`, and you have to *tell* a help page what to call the program.

`core/cli` is the opinionated half. A `Cli<C>` carries the name, the version,
the global `Flag`s and a list of `Command<C>`s. A command carries its own flags,
its declared `Arg`s **and the function that fires when you choose it**. That is
the arrangement `Server.onRequest` uses, so a library call dispatches rather
than the caller's `match` over a command name. One declaration therefore reads
four ways: the parse, the help page, the version line, and the message a refused
line earns. They cannot drift apart. `run(ctx, spec)` is the only exported
function and the one call a `main` needs. It reads the arguments, then prints
the automatic help or version page when someone asked for one, or fires the
command. A parse error goes to stderr with the usage under it and comes back as
`.Err`, which `main`'s contract turns into exit 1. A handler takes an
`Arguments` and asks it by name — `on`, `value`, `many`, `arg`, `positionals` —
rather than a struct of its own fields. `derive` only attaches a conformance to
a type that already exists, and one `Cli` holds *one* list of commands, so a
per-command argument struct has no type to be. Everything under `run` sits at
the `Alloc` tier, which lets a test hand it `core/host/testing`'s
`env().arguments([...])` and read the answer out of a captured stream.
`buri docs core/cli` is the module's own page, with the five spellings a flag
may take and a program worked end to end.

`core/net/server` is the other half of `core/net/http`: a program that *is* a
server rather than one that talks to one. That is a second authority, not a
second spelling.

A `Server<C, S>` holds the whole configuration. It carries a `port` and an
`onRequest` handler taking the caller's own context. `Option` knobs cover the
address, the protocols, a certificate, a request limit, an idle timeout, a
shutdown deadline, the WebSocket hooks and a socket buffer. Leave one out of the
literal and the runtime chooses. `S` is what one socket carries. On a server
with no hooks nothing constrains it, and the checker settles it as `()`, so a
program that does not do WebSockets never spells it. `serve` binds and answers
until the listener closes. `bind` and `run` split that in two, for a program
that wants the port number before it starts answering. `errorText` turns a
`ServeError` into a line.

It speaks HTTP/1.1, and HTTP/2 over TLS. A `tls: .Some(Tls { certificate,
key })` names two PEM *files*, read once when the port opens, so a certificate
that is missing or does not match its key stops the program starting. It turns
the server into an HTTPS one without changing a handler, which never learns
which transport its request arrived on. HTTP/2 comes with TLS and only with TLS,
because ALPN chooses it inside the handshake. So a `Server` naming `.Http2`
without a certificate fails at the bind rather than quietly serving HTTP/1.1,
and a `Server` with a certificate and no `protocols` offers HTTP/1.1, because
`.None` is the absence of a choice. An HTTP/1.1 connection carries one request.
Several share an HTTP/2 one, which is what multiplexing is. The server answers
as many at once as the acceptor said it would host, because `run` puts each
handler on a task of its own. That is why `serve` needs `Tasks` and `Alloc`
beside `Listen`. Only `LINUX` and `MACOS` grant `Listen`, because a page is
served rather than serving. `WEB` grants no `Tasks` either, so it refuses a
server on a page twice.

**A `Server` with a `websocket` speaks WebSockets, and the upgrade is
invisible.** With hooks present, a client that asks for a socket at the path the
hooks name gets one, and `onOpen` runs. Without them the same request reaches
`onRequest` like any other. Either way, a handler needs no branch.

`WebSocket.path` is a required field and not an `Option`, because a server that
upgraded every URL would have none left for anything else. The server compares
the request's path to it exactly. The query string plays no part and nothing is
normalised, so `"/socket"` and `"/socket/"` are two different paths. An upgrade
request to any other path is an ordinary request, and `onRequest` answers it
exactly as it would on a server whose `websocket` is `.None`.

`onOpen` answers the socket's first state, every later hook takes the current
one, and `onMessage` answers the next. So per-socket state is a value rather
than a table keyed by socket, and the counter-per-socket example in
`buri docs core/net/server` is an actor's address. A `Socket` is inert: one
integer, comparable, and sendable to an actor. That actor can push on it long
after the request that opened it returned, because `socket.send` and
`socket.close` need `C: Sockets` and nothing else. `send` never waits. It hands
the message to the socket's outbound buffer, and a buffer that fills closes the
socket with `.Overflow` and runs `onClose`. `socketBuffer` sets how deep that
buffer is. A close is a `CloseReason` and never a wire code, in both directions.
Ping and pong belong to the platform, so `onMessage` sees `.Text` and `.Binary`
and nothing else. A socket costs a worker: its whole life runs on the one that
accepted it. That makes the hooks on a socket run in order by construction, and
it means a server holding `listener.handlers` sockets has none left to accept
with. A socket still open when a shutdown begins closes with `.GoingAway`, so a
drain does not wait out a client that is doing nothing wrong, and `onClose`
still runs.

**A server stops gracefully.** `SIGTERM` and `SIGINT` do not kill a program
holding a port. The platform stops accepting connections, lets the program
answer the requests already in flight, then tells the accept loop the listener
is closed. `serve` returns `.Ok(())`, `main` falls off its end, and whatever a
program does after `serve` still happens. `drainMillis` bounds how long the
middle step may take. A second signal is the operating system's own, so
`Ctrl-C` twice stops a process that will not drain. None of this touches a
program with no listener open: the platform holds the signals only while it
holds a port.

`core/fs` is the one module that declares its own effects, and it declares
**two**. `FsRead` is four methods and `FsWrite` is eight. Reading and writing
are two grants rather than two spellings of one: a program that reads its
configuration has not thereby earned the right to delete it. A
`<C: Alloc + FsRead>` is a promise the compiler keeps for the whole call graph
below it. They live here rather than in `core/effect` because every method names
a `Path`, and `core/path` names `Alloc`. `core/fs` re-exports `Path`, so
`from "core/fs" import { FsRead, Path }` is one import. Beyond the wrappers over
those twelve methods it has two operations of its own. `readBytesIfExists` folds
`.NotFound` into `.None` in a single call, rather than the two an `exists` and a
read would take. `writeAtomic` is the write-sync-rename-sync sequence a
crash-safe checkpoint needs, written once.

`core/path` says where a file *is*, as a type. Every `Path` has been through
`path.of(ctx, text)`, so every one is spelled the one way: `"logs//app/"` and
`"logs/app"` are one path and compare equal. A filesystem operation taking a
`Str` would take any `Str`, including one an interpolation built with a
separator too many. The file that string does not open comes back `.NotFound`,
which reads exactly like a genuinely missing file. The type moves that
conversation to the call site. Normalizing drops empty and `.` components and a
trailing separator, and deliberately does **not** resolve `..`. Where `a` is a
symbolic link, `a/../b` and `b` name two different files, so `..` stays a
component and the filesystem decides what it means. `parent`, `fileName`,
`stem`, `extension` and `isAbsolute` are views and take no context. `of`, `join`,
`withSuffix` and `components` build something new and name `Alloc`. `join` never
substitutes an absolute argument for the receiver, so `path.of(ctx, "/srv").join(ctx, "/etc")`
is `/srv/etc`. The other behaviour is how a program that joined a user's string
onto its own directory ends up reading `/etc/passwd`.

`core/tasks` is one function. `parallel(ctx, items, f)` runs `f` over every item
and answers the results **in the items' order**, whatever order the work
finished in, handing each call the item's own index. Every task finishes before
`parallel` returns, so nothing outlives the context that granted it:

```buri
from "core/effect" import { Alloc, Tasks };
from "core/tasks" import * as tasks;

fn squares<C: Alloc + Tasks>(ctx: C, ns: [Int]): [Int] {
    tasks.parallel(ctx, ns, fn(c, i, n) => n * n)
}
```

The `c` a task takes is the **caller's whole context**: every effect `ctx`
carried. A task can do anything its caller could, and nothing it could not. It
arrives as a parameter because a lambda may not capture a context
(Section 10.6).

How much actually runs at once is the platform's business, not the signature's.
JavaScript starts the tasks together and awaits them together. A native
`--release` build gives each task a carrier of its own, so two that wait
overlap. `buri run` runs them in index order on one carrier, because a program
its backend builds has a single Buri stack to hold their frames in. All three
answer the same list, which is the point of fixing the order. Two tasks that
*compute* do not yet overlap on either native backend: `parallel` buys
overlapped waiting rather than more processors.

`core/actor` is the other half of concurrency: state that outlives one call,
behind a mailbox. An actor is a *value*, an initial state and a
`step: fn(C, S, M) => Stepped<S, R>`, and `start` gives it a mailbox and answers
an `Address`. Two enums are the protocol: `M` is what you may send and `R` is
what comes back, and a `Stepped` is what the step answers with — the state the
next message sees, and the answer this one gets. `sendMessage` posts a message
and hands that answer back. `stop` closes the mailbox, discards what is left,
and runs `onStop` once with the final state. Everything after that answers
`.Err(.Stopped)`.

```buri
from "core/actor" import { Actor, Stepped };

enum CounterMessage {
    Increment,
    Get,
}

fn counter<C>(initial: Int): Actor<C, Int, CounterMessage, Int> {
    Actor {
        state: initial,
        step: fn(c, count, message) => Stepped { state: count + 1, answer: count + 1 },
    }
}
```

It needs no test double, and that falls out of the shape rather than being an
omission: `step` is an ordinary function in an ordinary field, so you test an
actor by calling it. The mailbox holds sixty-four messages and you cannot
configure it. **The actor steps on the task that drives it.** `sendMessage` runs
the mailbox down before it answers, and `stop` before it runs `onStop`, so the
bound is what limits how much work may wait for a driver busy somewhere else.
That is a scheduling decision and not a semantic one, since the answers are the
same either way, exactly as `parallel`'s two arms answer the same list. But it
means an actor is not yet a way to get work done in the background.

`core/net/http` documents `Request` and `Response`, the two types `Net.fetch`
speaks in. It re-exports them from `core/effect`, where the effect's own
signature names them. You build a message with a free function and then by
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
each answer a *new* message, so you assemble a request by chaining and never by
mutation. The language has no associated functions, since a function inside an
`impl` block takes `self`, so the constructors are free functions:
`http.request`, `http.textRequest`, `http.status`, `http.ok`, `http.text`,
`http.json`.

`core/host/testing` is `core/host`'s surface for a test. It has the same names —
`alloc`, `stdout`, `stderr`, `stdin`, `fs`, `net`, `clock`, `rand`, `entropy`,
`env`, `proc`, `sockets` — but you **call** them rather than refer to them, so
each call mints a fresh double. A method configures one by answering a new one:
`clock().at(1000)`, `rand().seed(7)`, `entropy().seed(7)`,
`env().variables([...]).arguments([...])`, `fs().files([...]).readOnly()`.

`net()` **refuses** every request until `net().respond(fn(request) => ...)` says
what to answer. That responder is a pure function of the `Request`, because
SPEC 10.6 keeps it from capturing a context, so you build a response that needs
one beforehand and capture it.

`fs()`, `net()` and `stdin()` each log what they were asked. `calls()` answers
that log in the order the calls completed, and a test writes down what it
expects with the constructor of the same name: `readFile(path)`,
`writeFile(path, body)`, `fetch(request)`, `readBytes(n)`, one per method. Those
same constructors say what **breaks**.
`fs().faults([readFile(p).fails(.PermissionDenied)])` fails every matching call,
and `failsOnCall(n, e)` fails the `n`th. Success comes from the fixture and
failure comes from the plan, and a fault whose call never happens fails the
test.

`tasks()` is the one double whose subject is **scheduling** rather than state.
`Tasks.parallel` promises its results in the items' order and nothing about the
order the work runs in. So `tasks()` runs the tasks in program order,
`tasks().anyOrder()` runs them in the one order its own content seeds, and
`tasks().everyOrder()` runs the whole `test` body once per completion order. A
seed is the order's own number, so a failure names a line that replays it.

`sockets()` doubles the writing half of a WebSocket. `sockets().open()` mints a
`Socket` with no network behind it, `sent()` reads back `[(Socket, Message)]`,
and `isOpen(s)` says whether this double will still take a message for that
socket. So you test a broadcast room, which is `Sockets` and nothing else, with
no listener, no port and no client.

`entropy()` is the one double that is the *opposite* of what the effect
promises, and the only place in this language where these octets are predictable
on purpose. It draws from `rand()`'s own generator at `rand()`'s own seeds, so a
token minted in a test is a value you can write an assertion against, and it is
the same value on both backends. A program cannot reach it, because only a test
source may import `core/host/testing`, and a test cannot reach the real one. See
[testing](./build/testing.md).

## Allocators

[`core/alloc`](../../compiler/standard_library/sources/alloc.buri) —
`GeneralPurpose`, `Arena`, `FixedBuffer`. Three implementations of `Alloc`, and
anything may import them. `Alloc` is the one effect whose implementation carries
no authority: a `Region` is a number, so a library that builds its own allocator
has been granted nothing.

- **`GeneralPurpose`** — unbounded, counts. `gp.stats()` answers
  `Stats { allocations, bytes }`.
- **`FixedBuffer(n)`** — a byte budget, and charging past it **aborts**. Nobody
  chose that. It is forced: `allocate` answers `Region` and not
  `Result<Region, _>`, so there is no value to report a failure with, and
  [`language/expressions.md` §6.9](../language/expressions.md) says that is
  what an abort is for. The message carries both numbers.
- **`Arena`** — a separate counter, and nothing more than a counter. It does
  not free in bulk, and it says so.

`core/alloc` also has the **scope**:

```buri
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc };
from "core/fs" import * as fs;
from "core/fs" import { FsRead, Path };

fn inAScope<C: Alloc + FsRead>(ctx: C, at: Path): Bool {
    alloc.scoped(ctx, fn(c) => fs.exists(c, at))
}
```

`scoped(ctx, body)` runs `body` with a `Scoped<C>`, an attenuating wrapper that
forwards every effect `ctx` grants and replaces one. Its `Alloc` is the scope's
own arena. A charge inside reserves from that arena, and the caller's
allocator's totals do not move. When `body` returns, the arena's pages go back
to the platform. Nothing else changes: the body prints on the same stdout, reads
the same files, and fans out onto the same tasks.

It holds the **values** too. A `[Str]` a scope builds lives in the arena's own
pages, and they go back with the rest when `body` returns. So a scope is a
lifetime and not only a budget. One value leaves, `body`'s answer, and Buri
deep-copies it onto the caller's allocator first, at every depth: a nested list,
an enum's payload, a closure's captured environment. You never write that copy
and cannot observe it except as a cost.

Two consequences follow. **Answer only what you need**, because the copy is
proportional to what leaves: a scope that answers a whole parsed document copies
a whole parsed document. And **a task started inside a scope allocates outside
it**, on the ordinary heap. The arena belongs to the carrier that entered the
scope, and a step of a `Tasks.parallel` runs somewhere else.

An allocator hears about less than the cost model defines, identically on both
backends: **every `allocate(ctx, n)`, and nothing else.** The charge for an
operation is *defined* rather than measured. A `Str` of *n* UTF-8 bytes charges
`16 + n`, a `[T]` of *n* charges `16 + n * stride(T)`, and a view charges
nothing. The list and string rows are charged by definition and reported to no
allocator. The model sits beside `Alloc` in `core/effect`, where you meet it
while reading the effect.

## What is deliberately not here

- **Struct-of-arrays / `MultiArrayList`.** Not typeable today. Exposing "column
  *i* of `T`, at `T`'s *i*-th field type" needs dependent or row types, and
  [`language/types.md` §5.5](../language/types.md) has no records. Write the
  two-field struct yourself. On the JavaScript backend that is all a library
  would do.
- **Bulk reclamation outside a scope.** `scoped` frees in bulk because it knows
  when it is over and copies its answer out. `Arena`, the type you carry
  around, has no boundary to copy at, so it stays a counter. It answers "how
  much did parsing charge?" and reclaims nothing a `GeneralPurpose` would not
  have reclaimed anyway.
- **Automatic accounting of the list and string rows.** As above: the cost
  model defines them, and no allocator hears about them.

Why each of those stands where it does, and what would have to change, is
written where the machinery is.
[`design/native/MEMORY.md`](../../../../design/native/MEMORY.md) §7 covers the
cost model and the allocators.
[`design/non-goals.md`](../../../../design/non-goals.md) covers struct-of-arrays
and the type-generating `derive` it would need. Both are contributors'
documents, not a user's.
