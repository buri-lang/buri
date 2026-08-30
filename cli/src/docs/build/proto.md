# Importing a `.proto` schema

A `.proto` file in a package is a source, and the module it becomes is
generated rather than read:

```buri repo=cli/tests/conformance package=//lib/proto
from "//lib/proto/address.proto" import { Address, encodeAddress, decodeAddress };
from "core/effect" import { Alloc };
from "core/proto" import { ProtoError };

export fn roundTrip<C: Alloc>(ctx: C, a: Address): Result<Address, ProtoError> {
  decodeAddress(ctx, encodeAddress(ctx, a))
}
```

**The schema is an edition-2026 schema.** `syntax = "proto3"` is refused, and so
is proto2, and so is an older edition — see [Editions, and only
one](#editions-and-only-one) below.

The import path is the schema's own path, extension included:
`//lib/proto/address.proto` names `lib/proto/address.proto` on disk. There is
one spelling, it is the file's name, and nothing has to be learned about where
generated code lands — because it does not land anywhere. Nothing is written to
the source tree, there is no `_pb.buri` to check in and no step to forget to
run.

The rest of this page is the mapping, and the mapping is a promise: it is what
somebody else's program will find in the bytes.

## Declaring the schema

A `.proto` belongs to a rule the way a `.buri` does — through a field in
`BUILD.buri`:

```textproto schema=build
library {
  proto_sources: ["address.proto", "demo.proto"]
}
```

`buri gen` manages `proto_sources` exactly as it manages `sources`, and a
schema no rule lists is [`unused-library`](./cli.md), the
same error a stray `.buri` gets — with the fix naming `proto_sources` rather
than `sources`.

The generated module belongs to the declaring rule, so the library boundary
applies to it unchanged: `//lib/wire/point.proto` is internal to `//lib/wire`,
and another package reaches its types the way it reaches anything else, by
`lib.buri` re-exporting them. A schema exports everything it declares — that is
what a schema *is* — and `lib.buri` decides which of those names leave the
library.

One schema may `import` another, and both must belong to the same rule: the
generated modules import each other, and that import is subject to the boundary
like any other. Sharing a schema across packages therefore means re-exporting
its generated types from the owning library's `lib.buri`, which is the same
answer the boundary gives for a hand-written module.

Two hygiene rules step around a generated module on purpose. `unused-import`
and `dead-code` both ask a person to make an edit, and there is no
file here to edit: the module is a function of the schema.

## Editions, and only one

A schema declares `edition = "2026";`. Nothing else is accepted: not
`syntax = "proto3"`, not proto2, not edition 2023 or 2024, and not a file that
declares nothing.

That is a strong requirement and the reason is the next section. Editions
changed what a *singular field* means, and the change is not one a reader can
paper over: under proto3 a singular scalar has no presence, under editions it
has presence by default, and a file that says `proto3` is not a file this
mapping can read a little differently — it is a file it would read wrongly. So
it is refused, with the migration in the `fix`:

```text
error: `syntax = "proto3"` is not accepted [proto-syntax-declaration]
  = fix: migrate it: `edition = "2026";`, drop every `optional` and `required`
    label, and write `[features.field_presence = IMPLICIT]` on the fields that
    had none
```

One edition rather than a range, for the same reason there is one spelling of an
import path: a schema means one thing, and a reader that quietly accepted an
older set of feature defaults would decode the file in front of it as a
different file. Moving the requirement forward is one constant in
`build/protoschema.rs` — every feature that affects the wire or the JSON
resolves identically at editions 2023, 2024 and 2026, because protobuf gives a
feature the default of the closest edition at or before it and no such default
has been introduced since 2023.

## Messages, fields, and presence

```proto
edition = "2026";

package example.v1;

message Person {
  string name = 1;
  int32 age = 2 [features.field_presence = IMPLICIT];
  repeated string emails = 3;
  Address home = 4;
}
```

becomes

```text
export struct Person {
  export name: Option<Str>,
  export age: Int,
  export emails: [Str],
  export home: Option<Address>,
}

derive Eq, Show for Person;
```

| editions | Buri |
|---|---|
| `message` | `struct` with named fields, `derive Eq, Show` |
| singular `T` (the default, EXPLICIT presence) | `Option<T>` |
| singular `T` with `features.field_presence = IMPLICIT` | `T` |
| singular message field | `Option<T>`, whatever the feature says |
| `repeated T` | `[T]` |
| `oneof pick { ... }` | `enum Person_Pick`, held as `Option<Person_Pick>` |
| `message Outer { message Inner { } }` | `Outer` and `Outer_Inner`, side by side |
| `enum Colour` | `enum Colour`, value names verbatim, plus `Unrecognized(Int)` |

### Presence is the headline

**A singular field is `Option<T>`.** Editions made presence the default and
removed the `optional` label that used to ask for it, so this is not the
exception it was under proto3 — it is what a field is.

What that buys is that *absent* and *set to the zero value* are two different
messages, and both survive a round trip:

- `.None` is not written at all. Nothing on the wire, nothing in the JSON.
- `.Some(0)` is written — two bytes of it — and reads back as `.Some(0)`.

Under proto3 those were the same message, and one of them did not survive.
That is the whole difference, and it is why the requirement is worth a refusal
rather than a compatibility mode.

`features.field_presence = IMPLICIT` asks for the old behaviour, on a file, a
message, or a single field. An implicit field is a bare `T`; a value equal to
the type's default is indistinguishable from an absent one; and it is not
written when it holds that default. It is exactly what a proto3 singular field
was, which is what makes it the migration for one.

A singular *message* field is `Option<T>` whatever the feature says, and that is
protobuf's rule rather than this mapping's: a message field always tracks
presence, because there is no "default message" for an absent one to mean.

`LEGACY_REQUIRED` — the third value, which describes a proto2 `required` field —
is refused by name. A field that must be there is a promise the format cannot
keep across versions, which is why editions carries the value only to describe
files that already exist.

## Names

A field's Buri name is protoc's `json_name`: each `_` is dropped and the letter
after it capitalised, and nothing else changes case. `user_name` is `userName`
in the struct *and* in the JSON document, so there is one name to remember.

A field whose name collides with a Buri keyword gets a trailing underscore —
`type` becomes `type_` — and its JSON name is untouched, because the document
is not ours to rename.

Nested types flatten with an underscore, because Buri has no nested type
namespace to put them in: `Everything.Note` is `Everything_Note`, a
module-level declaration beside its parent. The name keeps the nesting visible,
which a bare `Note` would not, and a `oneof` named `contact` inside
`Everything` becomes `Everything_Contact` by the same rule.

## Scalars

| proto3 | Buri | |
|---|---|---|
| `int32` `int64` `sint32` `sint64` `uint32` `uint64` | `Int` | |
| `fixed32` `fixed64` `sfixed32` `sfixed64` | `Int` | |
| `double` `float` | `Float` | a `float` field rounds to binary32 on the way out |
| `bool` | `Bool` | |
| `string` | `Str` | UTF-8 on the wire; bytes that are not text are an error |
| `bytes` | `[U8]` | |

**The 64-bit caveat, on the JavaScript backend.** An `Int` is an `I64` on every
backend, and on the JavaScript one an `I64` is a double
([`core/num`](../guide/standard-library.md)), so it holds every integer up to 2^53
exactly and nothing above it. A `uint64` or `int64` field carrying a larger
value survives the round trip only to that precision. This is the same caveat
every double-backed protobuf implementation carries; it is stated here rather
than discovered, and it does not apply to a native build, where an `I64` is
sixty-four bits. What *is* true on every backend is that a `uint64` above 2^63
reads back negative, which is what a signed reading of those bits is.

Negative numbers cost bytes. A negative `int32` or `int64` is written as the
ten-byte varint of its 64-bit two's complement, exactly as protoc writes one. A
schema whose numbers are often negative should say `sint32`/`sint64`, which
zigzag first: -1 is one byte rather than ten.

## Enums

```proto
enum Shade {
  SHADE_UNSPECIFIED = 0;
  LIGHT = 1;
  DARK = 2;
}
```

becomes a Buri enum whose variants carry the proto value names verbatim —
`Shade.SHADE_UNSPECIFIED`, `Shade.DARK`. The names are kept as written because
proto3 JSON writes an enum as the *name* of its value, and renaming them here
would mean the document said one thing and the type said another.

An open enum's first value must be zero, and so does this reader require: the
zero value is what an unset field means.

**Editions enums are open, and an open enum keeps what it does not recognise.**
`features.enum_type` defaults to `OPEN`, whose contract is that a value the
schema does not name is part of the *value* rather than an unknown field — so
every generated enum carries one extra variant:

```text
export enum Shade {
  SHADE_UNSPECIFIED,
  LIGHT,
  DARK,
  Unrecognized(Int),
}
```

A message written by a newer schema therefore survives being read and written
again by an older one, which is the entire point of the rule. In JSON the
unrecognised value goes out as its number, which is what the mapping says. If a
schema already has a value called `Unrecognized`, that meaning wins and the
extra variant becomes `Unrecognized_`.

`features.enum_type = CLOSED` is refused by name. A closed enum makes an
unrecognised value an unknown *field*, which a generated struct has nowhere to
keep — so honouring it would mean silently losing what an open enum keeps.

## `oneof`

```proto
message Everything {
  oneof contact {
    string phone = 60;
    Address office = 61;
  }
}
```

becomes an enum of the cases, held as an `Option` — a `oneof` may be unset, and
`Option` is how Buri says so:

```text
export enum Everything_Contact {
  Phone(Str),
  Office(Address),
}

export struct Everything {
  export contact: Option<Everything_Contact>,
}
```

Presence is tracked for a oneof, so a case holding its own type's default is
still written: `.Some(.Phone(""))` puts an empty string on the wire and reads
back as itself, which `.None` does not.

## What comes with each type

For a message `M`, five functions, all exported:

```text
defaultM(): M                                    every field at its proto3 default
encodeM(ctx, M): [U8]                            the wire format
decodeM(ctx, [U8]): Result<M, ProtoError>
encodeMJson(ctx, M): Json                        the proto3 JSON mapping
decodeMJson(ctx, Json): Result<M, ProtoError>
decodeMJsonAt(ctx, Json, path): Result<M, ProtoError>
```

For an enum `E`: `encodeE(E): Int`, `decodeE(Int): E`, `encodeEJson(E): Json`,
and `decodeEJson(Json, path): Result<E, ProtoError>`.

`defaultM` is what makes a message with more than a few fields writable at all:

```buri repo=cli/tests/conformance package=//lib/proto
from "//lib/proto/demo.proto" import { Everything, Shade, defaultEverything };

export fn dark(): Everything {
  // Every singular field is an `Option`, because presence is the edition's
  // default — so setting one is `.Some(...)` and leaving it out is `.None`.
  Everything { ..defaultEverything(), name: .Some("Ada"), shade: .Some(Shade.DARK) }
}
```

`ProtoError` is [`core/proto`](../guide/standard-library.md)'s, and every case of it
carries a byte offset or a field number, because a decoder that says only
"malformed" of a four-kilobyte message is a decoder you debug by bisection.

### Why the codecs are generated Buri

`derive ToJson` is a walk over a *descriptor* the compiler already ships —
field names, variant shapes, element types — and the same walk would have been
the obvious thing to reuse here. It does not fit. A protobuf message is made of
field *numbers* and wire *types*, and the descriptor carries neither, so a
runtime walk would need a second, proto-specific descriptor emitted beside the
first. At that point generating Buri is strictly better: the generated codec is
checked by the real checker, optimised by the real optimiser, dead-code
eliminated with everything else, and needs no new intrinsic and no runtime
privilege. What is *shared* lives in `core/proto` — tags, wire types, packed
readers, the error type — so the generated part of a schema is only the part
that differs between schemas.

## The wire format

Ordinary proto3. Three things worth stating because a reader will otherwise
have to check:

- **A field that was set is written; a field that was not is not.** Under the
  edition's default presence that is all there is to it, and it is what makes
  `defaultM()` — every field `.None` — encode to zero bytes. An `IMPLICIT`
  field has no "was set", so the rule there is proto3's instead: it is written
  unless it holds the type's default.
- **A repeated numeric field is packed** — `features.repeated_field_encoding`
  defaults to `PACKED` — and `EXPANDED` asks for one whole field per element.
  A repeated `string`, `bytes`, or message field is never packed, whatever the
  feature says: packing strings would be indistinguishable from one long string.
  A reader accepts both forms of the numeric one, because a writer is allowed to
  send either.
- **A field the schema does not know is skipped**, which is proto3's forward
  compatibility and the whole of it. The skipped bytes are *dropped* rather
  than retained: a generated type has nowhere to keep them, so re-encoding a
  message decoded from a newer schema loses the fields that schema added. A
  known field arriving with a wire type it cannot have is skipped the same way,
  which is what keeps a reader that has not been rebuilt from crashing on a
  schema change.

A singular field that appears twice in one message takes the last occurrence.
For a scalar that is what the specification says; for a *message* field the
specification asks for a recursive merge instead, and last-wins is the rule a
generated struct with no mutation can express. Nothing this toolchain writes
produces such a message, so the difference is only visible reading one from a
writer that does.

Wire types 3 and 4 — proto2's groups — are refused rather than skipped. A group
is a nesting the reader would have to understand to get past, so treating one
as an unknown field would silently read the rest of the message at the wrong
offset.

Fields go out in schema order, so the same value is always the same bytes.

## The JSON mapping

`encodeMJson` writes proto3 JSON, which is **not** what `derive ToJson` writes.
Four differences, and each of them is why the codec is generated rather than
derived:

- A 64-bit integer is a **string**. A JSON number is a double, and
  `9007199254740993` is not one. A reader accepts a number as well, because the
  mapping asks a reader to be more permissive than a writer.
- `bytes` is **base64**, padded, not an array of numbers.
- An enum is the **name** of its value, not a tagged object. A number is
  accepted on the way in, and an unrecognised one is the zero value, exactly as
  in the binary format.
- A `oneof`'s selected case is an **ordinary member** of the enclosing object —
  `{"phone":"9"}`, not `{"contact":{"Phone":"9"}}`. The tagged form is what
  `derive ToJson` would write and what no other protobuf implementation reads.

Beyond those: a field that was not set is omitted and one that was is written —
including at its zero value, which is where explicit presence shows through into
JSON as well; an `IMPLICIT` field is omitted when it holds its default; an empty
repeated field is omitted rather than written as `[]`; an absent member and a
`null` member mean the same thing; and a member the schema does not know is
ignored.

One deviation from the specification, recorded rather than hidden: members are
written in schema order with a `oneof`'s case last, rather than strictly in
field-number order. JSON objects are unordered, no conforming reader can
notice, and the alternative is an interleave that buys nothing.

A failure names the path it happened at, written the way
[`core/json`](../guide/standard-library.md)'s is — `$` for the document, `.name` for a
member — so `$.home.city` is a place a reader can find in the text in front of
them.

## `google.protobuf.Any`, as its two fields

An `Any` is a message like any other in this mapping. Vendor the schema
[googleapis publishes](https://github.com/protocolbuffers/protobuf/blob/main/src/google/protobuf/any.proto),
declare it in a rule, and a field of that type reads as the struct the schema
says it is:

```text
export struct Any {
  export typeUrl: Str,
  export value: [U8],
}
```

That is the whole of it. **There is no unpacking.** `value` is the encoded
bytes of some other message, and which message the `type_url` names is a
question only a runtime type registry could answer — a table mapping a URL to a
decoder, populated by every schema linked into the program. This toolchain has
no such registry and is not getting one, so a program that wants what is inside
an `Any` decodes `value` itself with the codec for the type it expects:

```text
let inner = decodeErrorInfo(ctx, detail.value)?;
```

Two consequences, stated rather than discovered:

- **The JSON is the two-field object**, `{"typeUrl": "…", "value": "…"}` with
  the bytes base64 as any `bytes` field is. Canonical `Any` JSON inlines the
  held message and adds an `@type` member, and producing that needs the same
  registry the unpacking would. A document written here is read as an ordinary
  message by any implementation, and *not* as an `Any` by one that expects the
  canonical form.
- **Nothing checks the `type_url`.** It is a `Str` this toolchain neither
  validates nor resolves; whether the bytes match the URL is between the two
  programs exchanging them.

What the binary format does is exact: an `Any` written here is an `Any`
everywhere, because the wire encoding of the message is its two fields and
always was.

This is why `Any` is not in the table below. The constructs that *are* there
each ask for a semantics this mapping cannot express; `Any` only asks for a
struct, and dynamic dispatch was never in the message.

## What is not supported

Each of these is refused by name, with the reason and the edit, under
`proto-unsupported`:

| | Why not |
|---|---|
| `service`, `rpc` | This reader turns a schema into data types; there is no RPC transport to generate a stub against. |
| `extend`, `extensions` | An extension adds fields to a message from outside it, so the generated type would not be the whole of the message. |
| `group` | proto2's inline nesting, whose wire encoding was removed from proto3. Declare a nested `message`. |
| `map<K, V>` | Sugar for a repeated entry message with its own wire layout, and Buri's `Map` is not ordered the way a decoded map would have to be. Declare the entry message. |
| the `optional` and `required` labels | Editions removed both; presence is `features.field_presence` now. protoc refuses them in the same words. |
| `import public` | Re-exports another file's declarations, which would make one module's surface depend on a second file's. |
| `syntax = "proto2"`, `syntax = "proto3"`, editions before 2026 | See [Editions, and only one](#editions-and-only-one). |
| `features.field_presence = LEGACY_REQUIRED` | A field that must be there is a promise the format cannot keep across versions. |
| `features.enum_type = CLOSED` | An unrecognised value would become an unknown field, which a generated struct has nowhere to keep. |
| `features.message_encoding = DELIMITED` | The group encoding again, under its new name. |
| `features.utf8_validation = NONE` | A `string` field becomes a `Str`, and a `Str` is text — there is no unvalidated one for the bytes to become. Declare the field `bytes`. |
| `features.json_format = LEGACY_BEST_EFFORT` | It describes what proto2 did to JSON; this writes the one mapping editions defines. |
| `option features = { ... }` | The block form of a feature. One spelling of a thing is enough. |

`features.enforce_naming_style` and `features.default_symbol_visibility` are
source-retention lints — they say nothing about what a message means — so they
are read past rather than refused, which is what lets a schema opt out of the
naming style protoc enforces from edition 2024 on.

`option` and `reserved` are the two statements that are *skipped* rather than
refused: neither says anything about the shape of a message, and `option` in
particular is how a schema talks to code generators that are not this one.

An `import` inside a schema is written from the repository root, the way protoc
resolves one against `-I.`:

```proto
import "lib/proto/address.proto";
```

## Is it right?

Protobuf ships a conformance suite — a C++ runner that forks an implementation
and drives a few thousand wire-format and JSON edge cases at it — and Buri is
onboarded to it. `cli/tests/proto/` holds the vendored schemas, a testee that is
a Buri binary, and a failure list where every expected failure is filed under a
reason. It is the only test in this repository whose ground truth comes from
somewhere else.

```text
CONFORMANCE SUITE PASSED: 970 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

Every expected failure is filed under one of seven reasons in
[`cli/tests/proto/README.md`](../../../tests/proto/README.md), along with the
defects the suite found. One of the seven is worth naming here because it is
not a gap: the reference implementation is proto3 and the schema under test is
edition 2026, so the two disagree about whether a field set to its zero value is
written. They are both right about their own schema, and that difference is what
this page is mostly about.

The conformance run is not part of `cargo test`, because a suite that needs a
C++ build of another project is a suite that does not run.
`cli/tests/vectors/proto.rs` replays recorded exchanges through the same testee
under cargo, which is the half that can be hermetic.

## Caching

A schema is an input like any other. Its contents are in the declaring rule's
key, so editing one rebuilds exactly what depends on it and nothing else, and
`--explain` reports a `proto` action per rule that declares a schema:

```text
keyed  proto //lib/wire js a47062e1d851
keyed  compile //lib/wire js 13a53a25987e
run    link //cmd/app js 1c42f9658fa5
```

The action is `keyed` rather than `cached` for the same reason `compile` is:
this toolchain caches a binary's whole closure under one `link` key, so the
generated module has a key and no cache entry of its own. Saying so is better
than claiming a hit that did not happen.
