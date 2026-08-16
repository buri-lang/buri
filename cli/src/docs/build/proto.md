# Importing a `.proto` schema

A `.proto` file in a package is a source, and the module it becomes is
generated rather than read:

```buri repo=cli/tests/conformance pkg=//lib/proto
from "//lib/proto/address.proto" import { Address, encodeAddress, decodeAddress };
from "core/cap" import { Alloc };
from "core/proto" import { ProtoError };

export fn roundTrip<C: Alloc>(ctx: C, a: Address): Result<Address, ProtoError> {
  decodeAddress(ctx, encodeAddress(ctx, a))
}
```

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
schema no rule lists is [`undeclared-source`](./cli/src/docs/build/cli.md), the
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
and `unreachable-export` both ask a person to make an edit, and there is no
file here to edit: the module is a function of the schema.

## Messages, fields, and presence

```proto
syntax = "proto3";

package example.v1;

message Person {
  string name = 1;
  optional int32 age = 2;
  repeated string emails = 3;
  Address home = 4;
}
```

becomes

```text
export struct Person {
  export name: Str,
  export age: Option<Int>,
  export emails: [Str],
  export home: Option<Address>,
}

derive Eq, Show for Person;
```

| proto3 | Buri |
|---|---|
| `message` | `struct` with named fields, `derive Eq, Show` |
| `optional T` | `Option<T>` |
| `repeated T` | `[T]` |
| singular `T` (a scalar or an enum) | `T` |
| singular message field | `Option<T>` |
| `oneof pick { ... }` | `enum Person_Pick`, held as `Option<Person_Pick>` |
| `message Outer { message Inner { } }` | `Outer` and `Outer_Inner`, side by side |
| `enum Colour` | `enum Colour`, value names verbatim |

**A proto3 singular scalar is `T`, not `Option<T>`.** This is the one decision
the format forces and the language does not, so it is written here rather than
discovered. proto3 has *implicit presence*: a singular scalar is always there,
an unset one holds its type's default, and the wire format does not distinguish
"absent" from "set to zero" — a writer omits the default and a reader
substitutes it. Mapping such a field to `Option<T>` would invent a distinction
the bytes cannot carry: `.None` and `.Some(0)` would encode to the same message
and one of them would not survive a round trip. So the field is `T`, absence is
the default, and `optional` — which is proto3's own keyword for "presence is
tracked" — is the only thing that produces an `Option`.

A singular *message* field is the exception, and it is proto3's exception too:
a message field always tracks presence, because there is no "default message"
for an absent one to mean. So `Address home = 4;` is `Option<Address>` with no
`optional` in sight.

The consequence is worth stating: `Person { name: "" }` and a `Person` whose
name was never set are the same value, and both encode to zero bytes. If a
program needs to tell them apart, the schema says `optional string name = 1;`
and pays a byte for it.

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

**The 64-bit caveat.** An `Int` is an `I64` and an `I64` is a double
([`core/num`](./STANDARD-LIBRARY.md)), so it holds every integer up to 2^53
exactly and nothing above it. A `uint64` or `int64` field carrying a larger
value survives the round trip only to that precision, and a `uint64` above 2^63
reads back negative, which is what a signed reading of those bits is. This is
the same caveat every double-backed protobuf implementation carries; it is
stated here rather than discovered.

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

proto3 requires the first value to be zero, and so does this reader: the zero
value is the field's default, and it is what an unrecognised number decodes to.

**An unrecognised number decodes to the zero value.** That is a deliberate
loss. proto3 asks a reader to *keep* an unrecognised enum number so it survives
a re-encode; a Buri enum has nowhere to keep one. The alternative — failing —
would mean that adding a value to an enum broke every reader built before it,
which is the thing proto3's rule exists to prevent. Defaulting is the lesser of
the two, and it is why a schema's zero value should be a real "unspecified"
rather than a meaningful case.

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
  export Phone(Str),
  export Office(Address),
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

```buri repo=cli/tests/conformance pkg=//lib/proto
from "//lib/proto/demo.proto" import { Everything, Shade, defaultEverything };

export fn dark(): Everything {
  Everything { ..defaultEverything(), name: "Ada", shade: Shade.DARK }
}
```

`ProtoError` is [`core/proto`](./STANDARD-LIBRARY.md)'s, and every case of it
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

- **A singular field holding its default is not written.** That is proto3's
  canonical encoding, and it is what makes `defaultM()` encode to zero bytes.
  An `optional` field that is set is written whatever it holds.
- **A repeated numeric field is packed**; a repeated `string`, `bytes`, or
  message field is one whole field per element. A reader accepts both forms of
  the numeric one, because a writer is allowed to send either.
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

Beyond those: a field holding its default is omitted, an empty repeated field
is omitted rather than written as `[]`, an absent member and a `null` member
mean the same thing, and a member the schema does not know is ignored.

One deviation from the specification, recorded rather than hidden: members are
written in schema order with a `oneof`'s case last, rather than strictly in
field-number order. JSON objects are unordered, no conforming reader can
notice, and the alternative is an interleave that buys nothing.

A failure names the path it happened at, written the way
[`core/json`](./STANDARD-LIBRARY.md)'s is — `$` for the document, `.name` for a
member — so `$.home.city` is a place a reader can find in the text in front of
them.

## What is not supported

Each of these is refused by name, with the reason and the edit, under
`proto-unsupported`:

| | Why not |
|---|---|
| `service`, `rpc` | This reader turns a schema into data types; there is no RPC transport to generate a stub against. |
| `extend`, `extensions` | An extension adds fields to a message from outside it, so the generated type would not be the whole of the message. |
| `group` | proto2's inline nesting, whose wire encoding was removed from proto3. Declare a nested `message`. |
| `map<K, V>` | Sugar for a repeated entry message with its own wire layout, and Buri's `Map` is not ordered the way a decoded map would have to be. Declare the entry message. |
| `google.protobuf.Any` | Holds a message whose type is known only at runtime, and a generated type has to know its fields at compile time. Declare a `oneof`. |
| `required` | proto2's. A field that must be there is a promise the format cannot keep across versions. |
| `import public` | Re-exports another file's declarations, which would make one module's surface depend on a second file's. |
| `syntax = "proto2"` | The presence and default rules above are proto3's. |

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
CONFORMANCE SUITE PASSED: 988 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

The skips are the message types the testee does not implement — proto2 and the
editions variants — and the expected failures are dominated by two things the
vendored schema had removed from it, `map<>` and the well-known types, plus the
64-bit precision caveat above. `cli/tests/proto/README.md` lists all seven
reasons and the six real defects the suite found.

It is not part of `cargo test`, because a suite that needs a C++ build of
another project is a suite that does not run. `cli/tests/proto_vectors.rs`
replays recorded exchanges through the same testee under cargo, which is the
half that can be hermetic.

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
