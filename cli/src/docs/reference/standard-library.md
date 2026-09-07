# The standard library

The standard library ships with the toolchain. You never list it in a
`dependencies`, every target can use it, and nothing replaces it. It owns two
reserved module roots. `core/*` is a deliberately small set of essentials.
`ui/*` is the reactivity vocabulary, a much larger surface.

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
struct, so every operation in `core/simd` is pure.

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
[`core/proto`](../../compiler/standard_library/sources/proto.buri),
[`core/buri/ast`](../../compiler/standard_library/sources/buri_ast.buri),
[`core/codegen`](../../compiler/standard_library/sources/codegen.buri),
[`std/codegen/proto/schema`](../../compiler/standard_library/sources/codegen_proto_schema.buri),
[`std/codegen/proto`](../../compiler/standard_library/sources/codegen_proto.buri).

- **`core/str`** — a `Str` measures in Unicode scalar values everywhere. `len`
  counts them, `charAt` and `slice` index by them, and `compare` orders by them.
  Read that last one before you rely on it. `compare` uses scalar order, which
  is byte-for-byte UTF-8 order for a valid string, as in Rust, Go and Python. It
  is *not* the UTF-16 code-unit order a JavaScript `<` gives, on either backend,
  and the two disagree above the basic multilingual plane. `<`, `[Str].sort`,
  `core/order`'s `str` and an `OrdMap<Str, _>`'s key order all use that one
  comparison.

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

  The varints live here beside hex and base64 rather than in `core/proto`,
  because anything speaking a length-prefixed format needs the same one. They do
  64-bit arithmetic on two 32-bit halves, so a negative `int64` writes the ten
  bytes protoc writes, and every digit of a value past 2^53 survives on every
  backend.

  A **`Reader`** gives a name to the index that `readVarint(b, at)` threads.
  `takeByte`, `takeVarint`, `takeSlice` and `takeFramed` each answer the value
  and the *next* reader, rather than moving this one, so a decoder that looks
  ahead and changes its mind still holds the reader it started from. The methods
  that only move the cursor are pure. The two that answer a `[U8]` name `Alloc`,
  because a Buri list is a value and not a view, so slicing one copies. For the
  same reason there is **no `Builder`**: `[[U8]].flatten` is what building looks
  like here.

  `fromU64Be` and its seven relatives cover both ends of both widths, in both
  directions. Writing one is *pure*: an array literal of a fixed size allocates
  nothing a context has to grant. Reading answers an `Option`, because a number
  assembled out of octets that were not there is a wrong answer wearing the
  shape of a right one.

- **`core/json`** — a `Json` tree, `parse`, and `stringify`. **An object is an
  ordered association list, not a map**, so key order round-trips, nothing needs
  a `Hash` bound, and `get` costs O(n). Every number is a `Float`, which is what
  JSON says a number is. `MAX_DEPTH` caps nesting, because parsing recurses and
  the recursion is not in tail position.

  **`derive ToJson` and `derive FromJson` map it onto your own types**, and
  `encode` and `decode` are the two functions that use them. Both sit on
  `derive`'s fixed list, so no reflection and no macro is involved. The module's
  own source states how Buri shapes map onto JSON ones. Three of those decisions
  have something at stake: an enum is externally tagged, a positional struct is
  an array whatever its arity, and `Option<T>` is `T` or `null`. That last one
  means `Option<Option<T>>` does not round-trip.

  The compiler enforces that you **derive both traits and never write them by
  hand**, because a hand-written encoder would run where something encodes the
  type on its own and be skipped silently where something encodes a type holding
  it.

- **`core/proto`** — the protobuf wire format: tags, wire types, the packed
  readers, and `ProtoError`. You write none of this by hand either. A `.proto`
  schema in a package *becomes* a module, and this module is the part of that
  generated code that stays the same for every schema. See [the proto
  reference](./build/proto.md) for the mapping.

  `Stdin.readBytes` and `Stdout.writeBytes` are for reading a request and
  writing a reply over a pipe. `readLine` reads the stream to its end, so a
  program using it cannot answer before the other side has finished speaking.
  Text and octets are two questions about one stream, so a program should ask
  only one of them.

- **`core/buri/ast`** — the Buri grammar as Buri data, and `print`, which turns
  a `Module` back into source text. A generator builds the tree instead of a
  string, so it cannot emit a parse error, and every node carries the input
  span it came from. `print` answers the text and one anchor per node whose
  origin names a file — a byte range into the text, sorted by start, outermost
  first — which is what lets a diagnostic about generated code point at the
  input line behind it. A child position is a one-element array, because a Buri
  type recurses through `[T]`. Printing costs O(n) in the output text plus one
  UTF-8 measurement per piece written, and O(a log a) to sort the anchors. The
  output is what `buri format` leaves alone, with one limit: `print` has no page
  width, so it breaks only what the formatter always breaks and puts everything
  else on one line.

- **`core/codegen`** — the protocol a generator speaks. `run` reads one JSON
  line from `Stdin`, hands your function the `Request`, and writes the
  `Response` back as one JSON line on `Stdout`. It calls `core/buri/ast`'s
  `print` for you, so what goes over the wire is text plus anchors and never a
  tree. `main` names `Alloc`, `Stdin` and `Stdout` and nothing else, which is
  what makes a generator deterministic. `run` answers `.Err` when there was no
  request to read or the line was not one; returning that from `main` is how a
  generator fails visibly. Costs one parse of the request line plus one `print`
  per module — O(n) in the text read and the text written.

- **`std/codegen/proto/schema`** — a reader for `.proto` schemas, and the front
  half of the `std/codegen/proto` generator. `parse` answers what a file
  declares plus every diagnostic about it, each carrying the span in the schema
  that a `codegen.Diagnostic` points at. **One edition**: a schema says
  `edition = "2026";`, and `syntax = "proto3"`, proto2 and older editions are
  refused rather than read loosely. So is everything the mapping cannot express
  — `service`, `extend`, `group`, `map<>`, the removed labels, `import public`,
  and each unimplementable `features` value — refused *by name*, because a
  construct silently ignored makes a file mean something other than what it
  says. `option` and `reserved` are skipped. Costs one pass over the text, O(n),
  plus one `Int` per character: a span is measured in bytes and a `Str` in
  scalar values, so the offsets are computed once rather than per diagnostic.
  [The proto reference](./build/proto.md) is the mapping it feeds.

- **`std/codegen/proto`** — the other half: the schema `std/codegen/proto/schema`
  read, as a Buri module. `emit` is the generator `core/codegen`'s `run` takes,
  and `generate` is one schema at a time. It builds `core/buri/ast` nodes rather
  than text, and **every node carries the declaration behind it** — a struct its
  `message`'s span, a field's name the span of the `.proto` field, a variant the
  span of its value — which is what makes go-to-definition on a generated field
  land on the schema line that produced it. Each message brings `defaultM`,
  `encodeM`, `decodeM`, `encodeMJson`, `decodeMJson` and `decodeMJsonAt`, and
  each enum four of its own. Costs one pass over the schema to build the type
  table and one to write the tree; a type name resolves through an `OrdMap`, so
  a schema of `n` declarations costs O(n log t) in the `t` types in scope.
  [The proto reference](./build/proto.md) is the mapping, and it is a promise.

## Collections

[`core/list`](../../compiler/standard_library/sources/list.buri),
[`core/queue`](../../compiler/standard_library/sources/queue.buri),
[`core/map`](../../compiler/standard_library/sources/map.buri),
[`core/set`](../../compiler/standard_library/sources/set.buri),
[`core/ordmap`](../../compiler/standard_library/sources/ordmap.buri),
[`core/ordset`](../../compiler/standard_library/sources/ordset.buri),
[`core/bitset`](../../compiler/standard_library/sources/bitset.buri).

Every one of these is a value, so every "modification" answers a new one. Each
module states its cost rather than leaving you to guess:

| | Lookup | Insert | Note |
|---|---|---|---|
| `core/queue` | O(1) | O(1) amortized | Banker's deque: two lists, the front reversed. The reversal makes both ends an append. |
| `core/map`, `core/set` | O(1) expected | O(b) in buckets | Buckets of association lists. Grows and rehashes past a load factor of 4. **Iteration order is unspecified and will change.** |
| `core/ordmap`, `core/ordset` | O(log n) | O(log n) | A persistent B-tree, seven entries to a node. **Iteration runs in key order.** `range` and `prefix` scan at O(log n + m) rather than filtering over everything. |
| `core/bitset` | O(1) | O(n/32) | 32 bits to an `Int` word. 32 and not 64 because `Int` is signed, and a bit in position 63 would make every shift a question about sign extension. |

**Two keyed collections, and order is what you choose between.** `Map` hashes,
and looks one key up faster. `OrdMap` compares, and answers "every key between
these two" or "every key starting with this" without visiting the rest. Its keys
need `Ord` rather than `Hash + Eq`. A compound key is a struct with `derive
Ord`, and a derived `Ord` compares fields in declaration order, which is what a
multi-column index wants.

**A fallible step is a traversal, not a fold.** `xs.mapResult(ctx, f)` and
`mapOption` map every element, or stop at the first that fails. `filterMap` maps
and filters in one pass. Each has a `*Ctx` form that hands the step the context,
because a validation usually allocates as it goes and a lambda may not capture a
context. Beside them in `core/list`: `removeAt`, `windows`, `generate`,
`uniqueBy`, `isSortedBy`, `maxBy`/`minBy` and `compareBy`. `uniqueBy` keeps the
first of each equal class, so it costs O(n²) in comparisons. Where the order may
change, `sortBy` and a walk is the O(n log n) answer.

**Grouping answers a map, so it lives with the map.** `map.groupBy(ctx, xs,
key)` and `ordmap.groupBy` collect the elements under each key, and `indexBy`
keeps one element per key. They are free functions because `core/list` sits at
the bottom of the dependency order and cannot name a map. `OrdMap.alter` is
insert, replace and remove in one call, which is what a counter needs.
`OrdMap.mapValues` puts every value through a function without touching the
keys, rebuilding the tree as it goes, which costs O(n log n).

`Queue`, `Map`, `Set`, `OrdMap`, `OrdSet` and `BitSet` provide `equals` rather
than deriving `Eq`, because a derived `Eq` would compare the *representation*.
Two maps built in different orders need not share a bucket layout.

## Numbers and vectors

[`core/simd`](../../compiler/standard_library/sources/simd.buri) — `F32x4` and `I32x4`.

**On the JavaScript backend these are scalar and buy no speed.** What they buy
is the shape. A kernel written lane-wise, with no loop-carried dependency, is
the form a backend with vector registers can lower directly. The same kernel
written as a fold over a list is not, because a fold says "in this order". Do
not benchmark against a scalar loop expecting a win.

## Time

[`core/time`](../../compiler/standard_library/sources/time.buri) is the clock,
and reading it performs an effect.
[`core/date`](../../compiler/standard_library/sources/date.buri) is the calendar,
and none of it performs one: what day of the week a date falls on does not
depend on anything.

`core/date` uses Hinnant's `days_from_civil`. It does integer arithmetic only
and stays exact over the whole range of `Int`.

**`Duration` and `Instant` both live in `core/time`.** A `Duration` is a length
and an `Instant` is a point, and they are different types on purpose. They share
a module because you may only declare a method in its receiver's defining
module, and `instant.plus(duration)` has to live somewhere. `core/date`
re-exports `Duration`, so `from "core/date" import { Duration }` still resolves
to the same type.

A `Duration` counts **nanoseconds**. An `Instant` counts milliseconds, which is
what the clock reports. `time.seconds(30)`, `millis`, `micros`, `nanos`,
`minutes` and `hours` build one, and `add`, `sub`, `mul`, `negate` and `abs`
combine them. **Every one of those saturates**, because overflow is undefined
behaviour and a deadline is where a program can least afford it.
`instant.hasPassed(deadline)` is that whole check. Its `Show` prints `1.5s`,
`300ms`, `750us` or `1ns`: the largest unit the length reaches, with the exact
fraction. There is no `m` or `h`, because a fraction of an hour is not a decimal.

**There is no timezone database, and there will not be one.** tzdata runs to
megabytes and changes several times a year, and this toolchain has no
dependencies and ships no data files. `Zoned` carries a fixed offset in
minutes, which covers UTC, a stored offset, and arithmetic within one offset.
It does not cover `America/New_York`, and it does not pretend to.

## Randomness

[`core/random`](../../compiler/standard_library/sources/random.buri) has two
doors, and the split follows one principle: **an RNG either takes a seed or
takes a context**.

`int`, `float` and `bytes` take a context and perform the `Rand` effect. `Gen`
takes a seed and performs nothing. `random.seeded(7)` is an ordinary value,
every method answers `(value, Gen)`, and the same seed gives the same sequence
on every backend and in every process. `Gen` is splitmix64, published in the
module rather than hidden behind an effect. `split()` answers two streams, where
a program would otherwise invent salt constants by hand.

`random.gen(ctx)` bridges the two: draw a seed from the platform once, then stay
pure. That is what a deterministic simulator needs, since it replays a failure
from a seed and cannot take its generator from whoever called it.

`Gen.nextInt` rejection-samples, so it has **no modulo bias**.

Neither door is a secret. Both are uniform and both are predictable. For octets
nobody can guess, see [`core/crypto`](#cryptography) below.

## Checksums

[`core/hash`](../../compiler/standard_library/sources/hash.buri) — `fnv1a32`,
`fnv1a64`, `crc32c` and `siphash24`, pure over `[U8]`.

**This is not [`core/crypto`](#cryptography), and that is the whole reason it is
a module of its own.** Nothing here is a digest: given a target value, producing
a message that hashes to it is arithmetic rather than work. Use these where a
digest is the wrong size — a flipped bit in a log record, a bucket index, a
fingerprint you can compare two runs of a simulator on.

Write `crc32c` into a record, because a storage format's readers already expect
it. `siphash24` is the only one that takes a key, and the key is the point. Put
an unkeyed hash behind a map whose keys arrive from outside, and someone can
hand you a thousand keys that all land in one bucket.

Each one is written in Buri and pinned to the vectors its publisher wrote down:
Noll's for FNV-1a, the CRC-32C check value and RFC 3720 B.4's iSCSI cases, and
the SipHash-2-4 reference table. So the answer is the same on both backends, and
it is the answer another implementation gives.

## Cryptography

[`core/crypto`](../../compiler/standard_library/sources/crypto.buri) — SHA-256,
HMAC-SHA-256, a constant-time comparison, and the platform's cryptographic
randomness.

The hashes are written in Buri rather than handed to the platform, because a
dependency tree is a second thing to audit. The NIST vectors check them, and
check the independent SHA-256 the build cache uses.

`randomBytes` and `token` are the half *not* written here. They perform the
`Entropy` effect, and the operating system supplies the octets: `getrandom(2)`
and `getentropy(2)` under a native binary, `crypto.getRandomValues` under a
JavaScript one. It is an effect rather than a plain function so that a program
has to *ask* for unguessability, and can be refused where it cannot be had.

**`randomBytes` is in `core/crypto` and not in `core/random`, deliberately.**
`core/random` is seeded and reproducible on purpose. You cannot tell the two
sets of octets apart by inspection — they differ only in whether an observer can
predict the next one — so a program says which it meant by the module it
imports. `token(ctx, 32)` is the spelling for a session's resume token: 32
octets of entropy, written as lowercase hex.

Every platform grants `Entropy`. What can be missing is the *toolchain*: a
runtime archive built without its `crypto` feature refuses `randomBytes` by
name, before code generation, rather than answering from a generator that is
merely uniform. See
[cryptography-not-available](./errors/cryptography-not-available.md).

Deliberately absent, and not by oversight:

- **No ciphers, yet.** Ship a block function without a key schedule, a mode, a
  nonce discipline and an authentication tag, and people end up with ECB. It
  would take the shape of one authenticated construction: an AEAD, with
  `Entropy` minting the nonce rather than the caller. It needs a second host
  effect on both backends, `crypto.subtle` is undefined on a page that is not a
  secure context, and every Buri function that could reach it becomes `async` in
  the emitted JavaScript.
- **No public-key anything.**
- **No key derivation and no password hashing.**

`sha256` is **not a password hash**. It is fast, which is the wrong property. It
is also the wrong size for a flipped-bit guard: thirty-two octets of frame on a
log record where four would do. [`core/hash`](#checksums) covers that case.

## User interfaces

[`ui/effect`](../../compiler/standard_library/sources/ui_effect.buri),
[`ui/signal`](../../compiler/standard_library/sources/ui_signal.buri),
[`ui/prop`](../../compiler/standard_library/sources/ui_prop.buri),
[`ui/node`](../../compiler/standard_library/sources/ui_node.buri),
[`ui/style`](../../compiler/standard_library/sources/ui_style.buri),
[`ui/theme`](../../compiler/standard_library/sources/ui_theme.buri),
[`ui/web`](../../compiler/standard_library/sources/web.buri) and
[`ui/testing`](../../compiler/standard_library/sources/ui_testing.buri) are the
second reserved root. They have a page of their own:
[user interfaces](../guides/user-interfaces.md).

Two of them answer what a tree *looks* like. `ui/node`'s `describe` resolves one
to a scene document, and `ui/testing`'s `snapshot` paints that document and
holds the PNG to a golden checked in beside the suite. The toolchain paints it
itself, so neither needs a browser.

`ui/web` is the same tree on a server. A worker renders it to HTML and sends
the state it rendered from with it; the page takes that markup over and reads
the state back out.

```buri
from "core/effect" import { Alloc };
from "core/json" import { Json };
from "core/net/http" import * as http;
from "core/net/http" import { Response };
from "ui/node" import * as ui;
from "ui/node" import { Node };
from "ui/prop" import { Prop };
from "ui/web" import * as web;

fn page<C>(path: Prop<Str>): Node<C> {
    ui.region(.Main, [], [ui.heading(1, path)])
}

fn answer<C: Alloc>(ctx: C, path: Str, state: Json): Response {
    http.html(ctx, web.shell(ctx, web.render(page(.Const(path))), state))
}
```

`render` takes no context and cannot need one: every constructor in `ui/node` is
unbounded in `C`, so nothing in a tree can act while it is being written out.
`shell` puts the state in an inert `<script id="buri-state">` and the compiler's
stylesheet in the head. On the page, `web.resume(ctx)` picks that state up —
`web.state(ctx)` reads it — and renders nothing, so the reader keeps looking at
the markup that arrived.

Routing is a match. A page function takes the path as a `Prop<Str>`: the worker
passes `.Const(request.path())` and the page passes `web.route(ctx)`, which is
the address bar as a cell. `Location` is granted on WEB alone, and it is what
makes navigating re-run the smallest thing that read the path.
[Build a website](../guides/websites.md) walks both halves end to end.

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
[`core/net/websocket`](../../compiler/standard_library/sources/websocket.buri),
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
There is no `assert.fail`: it answered `()` rather than a bottom type, so a
match arm using it could not produce a value.

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
its declared `Arg`s **and the function that fires when you choose it**. So one
declaration reads four ways — the parse, the help page, the version line, and
the message a refused line earns — and they cannot drift apart. `run(ctx, spec)`
is the only exported function and the one call a `main` needs. It reads the
arguments, then prints the automatic help or version page when someone asked for
one, or fires the command. A parse error goes to stderr with the usage under it
and comes back as `.Err`, which `main`'s contract turns into exit 1. A handler
takes an `Arguments` and asks it by name — `on`, `value`, `many`, `arg`,
`positionals` — rather than a struct of its own fields. Everything under `run`
sits at the `Alloc` tier, which lets a test hand it `core/host/testing`'s
`env().arguments([...])` and read the answer out of a captured stream.
`buri docs core/cli` is the module's own page, with the five spellings a flag
may take and a program worked end to end.

`core/net/server` is the other half of `core/net/http`: a program that *is* a
server rather than one that talks to one. That is a second authority, not a
second spelling.

A `Server<C, S>` holds the whole configuration. It carries a `port` and an
`onRequest` handler taking the caller's own context. `Option` knobs cover the
address, the protocols, a certificate, a request limit, an idle timeout, a
shutdown deadline, the WebSocket hooks and a socket buffer; leave one out of the
literal and the runtime chooses. `S` is what one socket carries, and the checker
settles it as `()` on a server with no hooks, so a program that does not do
WebSockets never spells it. `serve` binds and answers until the listener closes.
`bind` and `run` split that in two, for a program that wants the port number
before it starts answering. `errorText` turns a `ServeError` into a line.

It speaks HTTP/1.1, and HTTP/2 over TLS. A `tls: .Some(Tls { certificate,
key })` names two PEM *files*, read once when the port opens, so a certificate
that is missing or does not match its key stops the program starting. A handler
never learns which transport its request arrived on. HTTP/2 comes with TLS and
only with TLS, because ALPN chooses it inside the handshake: a `Server` naming
`.Http2` without a certificate fails at the bind, and one with a certificate and
no `protocols` offers HTTP/1.1. The server answers as many requests at once as
the acceptor said it would host, because `run` puts each handler on a task of
its own, which is why `serve` needs `Tasks` and `Alloc` beside `Listen`. Only
`LINUX` and `MACOS` grant `Listen`, and `WEB` grants no `Tasks` either.

**A `Server` with a `websocket` speaks WebSockets, and the upgrade is
invisible.** With hooks present, a client that asks for a socket at the path the
hooks name gets one, and `onOpen` runs. Without them the same request reaches
`onRequest` like any other. Either way, a handler needs no branch.

`WebSocket.path` is a required field and not an `Option`, because a server that
upgraded every URL would have none left for anything else. The server compares
the request's path to it exactly. The query string plays no part and nothing is
normalised, so `"/socket"` and `"/socket/"` are two different paths. An upgrade
request to any other path is an ordinary request.

`onOpen` answers the socket's first state, every later hook takes the current
one, and `onMessage` answers the next. So per-socket state is a value rather
than a table keyed by socket. A `Socket` is inert: one integer, comparable, and
sendable to an actor. That actor can push on it long after the request that
opened it returned, because `socket.send` and `socket.close` need `C: Sockets`
and nothing else. `send` never waits. It hands the message to the socket's
outbound buffer, and a buffer that fills closes the socket with `.Overflow` and
runs `onClose`. `socketBuffer` sets how deep that buffer is. A close is a
`CloseReason` and never a wire code, in both directions. Ping and pong belong to
the platform, so `onMessage` sees `.Text` and `.Binary` and nothing else. A
socket costs a worker: its whole life runs on the one that accepted it, so the
hooks on a socket run in order by construction, and a server holding
`listener.handlers` sockets has none left to accept with. A socket still open
when a shutdown begins closes with `.GoingAway`, and `onClose` still runs.

**A server stops gracefully.** `SIGTERM` and `SIGINT` do not kill a program
holding a port. The platform stops accepting connections, lets the program
answer the requests already in flight, then tells the accept loop the listener
is closed. `serve` returns `.Ok(())`, and whatever a program does after `serve`
still happens. `drainMillis` bounds how long the middle step may take, and a
second signal is the operating system's own, so `Ctrl-C` twice stops a process
that will not drain. The platform holds the signals only while it holds a port.

`core/net/websocket` is the client half of the same socket: a program that
*dials* one somebody else is holding. It is the same three hooks over the same
`Socket`, `Message` and `CloseReason`, which it re-exports from
`core/net/server`, so one end reads like the other.

A `Client<C, S>` carries a `url` and the hooks. `connect(ctx, client)` dials,
runs `onOpen`, runs `onMessage` for every frame, runs `onClose`, and answers the
`CloseReason` the socket ended with. It returns *when the socket closes*, so
reconnecting is a loop around it with `time.sleepMs` in the retry, and backoff
is your own arithmetic rather than a knob. An `.Err` is a socket that never
opened — a URL this platform cannot dial, a machine that refused, a server that
did not answer `101`. Everything after that is an `.Ok`, because a socket
closing is the ordinary end of one.

`onOpen` is handed the `Response` that opened the socket where a server's is
handed the `Request` that asked, and that is the whole difference. It is where a
negotiated subprotocol arrives, and on `LINUX` and `MACOS` it is the head the
server really sent; a page cannot see its own handshake, so there `Response`
carries the subprotocol and the extensions and nothing else.

`LINUX` and `MACOS` write the handshake here and check every clause of the
answer, so a `101` signing another handshake's key is `.Err(.Transport)` naming
that check. Everywhere else the engine's own `WebSocket` owns the handshake and
decides how strictly to check it — `node` refuses that `101` and `bun` accepts
it — because a page never sees the key it sent.

`connect` is bounded `WebSocketClient + Sockets`. The first dials and the second
pushes, and the hooks are handed your context, so both have to be in it.
**Every platform grants both**, `WEB` and `CLOUDFLARE_WORKER` included: holding
a port open is a native program's authority, and dialling out is not. On a page
`connect` follows `ui.mount` — it suspends without holding the event loop, so an
interface goes on rendering while the socket is idle and a pushed frame wakes it
like a click. A worker dials the same way while it answers a request.

`Client` has no header list, because a browser's `WebSocket` cannot send request
headers. A token or a subprotocol goes in the URL, which is what every browser
client does, and what came back is on the response in `onOpen`.

`core/fs` is the one module that declares its own effects, and it declares
**two**. `FsRead` is four methods and `FsWrite` is eight. Reading and writing
are two grants rather than two spellings of one: a program that reads its
configuration has not thereby earned the right to delete it. `core/fs`
re-exports `Path`, so `from "core/fs" import { FsRead, Path }` is one import.
Beyond the wrappers over those twelve methods it has two operations of its own.
`readBytesIfExists` folds
`.NotFound` into `.None` in a single call, rather than the two an `exists` and a
read would take. `writeAtomic` is the write-sync-rename-sync sequence a
crash-safe checkpoint needs, written once.

`core/path` says where a file *is*, as a type. Every `Path` has been through
`path.of(ctx, text)`, so every one is spelled the one way: `"logs//app/"` and
`"logs/app"` are one path and compare equal. Without the type, a string an
interpolation built with a separator too many opens nothing and comes back
`.NotFound`, which reads exactly like a genuinely missing file. Normalizing
drops empty and `.` components and a trailing separator, and deliberately does
**not** resolve `..`. Where `a` is a symbolic link, `a/../b` and `b` name two
different files, so `..` stays a component and the filesystem decides what it
means. `parent`, `fileName`, `stem`, `extension` and `isAbsolute` are views and
take no context. `of`, `join`, `withSuffix` and `components` build something new
and name `Alloc`. `join` never substitutes an absolute argument for the
receiver, so `path.of(ctx, "/srv").join(ctx, "/etc")` is `/srv/etc`. The other
behaviour is how a program that joined a user's string onto its own directory
ends up reading `/etc/passwd`.

`core/tasks` has two shapes. `parallel(ctx, items, f)` runs `f` over every item
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
overlap. `buri run` runs them in index order on one carrier. All three answer
the same list. Two tasks that *compute* do not yet overlap on either native
backend: `parallel` buys overlapped waiting rather than more processors.

`scope` and `spawn` are the other shape: work that runs beside the code that
started it. A scope returns when its body **and every task spawned into it**
have finished, so the waiting moves from the call to the scope and nothing
still escapes the context that granted it:

```buri
from "core/effect" import { Alloc, Clock, Stdout, Tasks };
from "core/io" import * as io;
from "core/tasks" import * as tasks;
from "core/time" import * as time;

fn page<C: Alloc + Clock + Stdout + Tasks>(ctx: C): () {
    tasks.scope(ctx, fn(c, here) => {
        let _ = tasks.spawn(c, here, fn(c2) => {
            let _ = time.sleepMs(c2, 5 * 60 * 1000);
            let _ = io.println(c2, "sessions expired").ignore();
            ()
        });
        ()
    })
}
```

A timer is a task that sleeps — there is no `Timer` and no `setTimeout`. A
`Scope` is inert, so a lambda may capture one, which is how an interface hands
a scope to a handler that spawns later. A library cannot spawn: it exposes a
`run` and the application puts it in a scope. And stopping is cooperative —
a loop ends by finding its socket closed or by asking an actor whether to carry
on, because there is no way to unwind a task from outside it.

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

It needs no test double: `step` is an ordinary function in an ordinary field, so
you test an actor by calling it. The mailbox holds sixty-four messages and you
cannot configure it. **The actor steps on the task that drives it.**
`sendMessage` runs the mailbox down before it answers, and `stop` before it runs
`onStop`. So an actor is not yet a way to get work done in the background.

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
`http.json`, `http.html`.

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
socket. So you test a broadcast room with no listener, no port and no client.

`sockets().dialling(messages)` doubles the reading half, for a client. It is a
`sockets()` and a script: `connect` dials it, gets a socket of *that* double's,
receives those messages in order, and closes normally when the script runs out.
So the pushes a client makes land in `sent()` and the whole of
`core/net/websocket` runs with no network at all. Every dial replays the script
on a socket of its own, so a reconnect loop gets a second session rather than a
socket that was already spent. A URL that is neither `ws://` nor `wss://` is the
refusal — `.Err(.Unsupported)`, the cause a real client gives a scheme it cannot
speak — which is how you test what your program does when the socket never
opens.

`entropy()` is the one double that is the *opposite* of what the effect
promises, and the only place in this language where these octets are predictable
on purpose. It draws from `rand()`'s own generator at `rand()`'s own seeds, so a
token minted in a test is a value you can write an assertion against, and it is
the same value on both backends. A program cannot reach it, because only a test
source may import `core/host/testing`. See [testing](./build/testing.md).

## Allocators

[`core/alloc`](../../compiler/standard_library/sources/alloc.buri) —
`GeneralPurpose`, `Arena`, `FixedBuffer`. Three implementations of `Alloc`, and
anything may import them. `Alloc` is the one effect whose implementation carries
no authority: a `Region` is a number, so a library that builds its own allocator
has been granted nothing.

- **`GeneralPurpose`** — unbounded, counts. `gp.stats()` answers
  `Stats { allocations, bytes }`.
- **`FixedBuffer(n)`** — a byte budget, and charging past it **aborts**. That is
  forced: `allocate` answers `Region` and not `Result<Region, _>`, so there is
  no value to report a failure with, and
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
pages, so a scope is a lifetime and not only a budget. One value leaves,
`body`'s answer, and Buri deep-copies it onto the caller's allocator first, at
every depth: a nested list, an enum's payload, a closure's captured environment.

Two consequences follow. **Answer only what you need**, because the copy is
proportional to what leaves. And **a task started inside a scope allocates
outside it**, on the ordinary heap: the arena belongs to the carrier that
entered the scope, and a step of a `Tasks.parallel` runs somewhere else.

An allocator hears about less than the cost model defines, identically on both
backends: **every `allocate(ctx, n)`, and nothing else.** The charge for an
operation is *defined* rather than measured. A `Str` of *n* UTF-8 bytes charges
`16 + n`, a `[T]` of *n* charges `16 + n * stride(T)`, and a view charges
nothing. Those rows are charged by definition and reported to no allocator. The
model sits beside `Alloc` in `core/effect`.

## What is deliberately not here

- **Struct-of-arrays / `MultiArrayList`.** Not typeable today. Exposing "column
  *i* of `T`, at `T`'s *i*-th field type" needs dependent or row types, and
  [`language/types.md` §5.5](../language/types.md) has no records. Write the
  two-field struct yourself.
- **Bulk reclamation outside a scope.** `scoped` frees in bulk because it knows
  when it is over and copies its answer out. `Arena`, the type you carry
  around, has no boundary to copy at, so it stays a counter.
- **Automatic accounting of the list and string rows.** As above: the cost
  model defines them, and no allocator hears about them.

Why each of those stands where it does, and what would have to change, is
written where the machinery is.
[`design/native/MEMORY.md`](../../../../design/native/MEMORY.md) §7 covers the
cost model and the allocators.
[`design/non-goals.md`](../../../../design/non-goals.md) covers struct-of-arrays
and the type-generating `derive` it would need.
