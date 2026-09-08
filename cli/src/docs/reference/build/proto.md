# Importing a `.proto` schema

A `.proto` file in a package is a source, and the compiler generates the module
it becomes rather than reading one. For the task, read [import a `.proto`
schema](../../guides/proto.md). This page is the mapping, and the mapping is a
promise: it is what somebody else's program will find in the bytes.

**The schema is an edition-2026 schema.** Buri refuses `syntax = "proto3"`,
proto2, and any older edition. See [Editions, and only
one](#editions-and-only-one) below.

The import path is the schema's own path, extension included:
`//lib/proto/address.proto` names `lib/proto/address.proto` on disk. Nothing
reaches the source tree, so there is no `_pb.buri` to check in and no step to
forget to run.

## Declaring the schema

A `.proto` is a generator's input, and `std/codegen/proto` is the generator:

```textproto schema=build
library {
    generators: [
        {
            tool: "std/codegen/proto"
            inputs: ["address.proto", "demo.proto"]
        }
    ]
}
```

`generators` is hand-authored — `buri gen` cannot know which generator owns a
file, so it never writes the field. A schema no entry lists is
[`unused-library`](../lints/unused-library.md), the same finding a stray `.buri`
gets. The old spelling, `proto_sources`, is
[retired](../errors/retired-proto-sources.md).

The generated module belongs to the declaring rule, so the library boundary
applies to it unchanged. `//lib/wire/point.proto` is internal to `//lib/wire`,
and another package reaches its types through `lib.buri` re-exporting them. A
schema exports everything it declares, because that is what a schema *is*, and
`lib.buri` decides which of those names leave the library.

One schema may `import` another, and both must belong to the same rule. So
sharing a schema across packages means re-exporting its generated types from the
owning library's `lib.buri`.

`unused-import` and `dead-code` step around a generated module. Both ask a
person to make an edit, and here there is no file to edit.

## Editions, and only one

A schema declares `edition = "2026";`. Buri accepts nothing else: not
`syntax = "proto3"`, not proto2, not edition 2023 or 2024, and not a file that
declares nothing.

Editions changed what a *singular field* means, and no reader can paper over the
change. Under proto3 a singular scalar has no presence; under editions it has
presence by default. Buri refuses an older file and puts the migration in the
`fix`:

```text
error: `syntax = "proto3"` is not accepted [proto-syntax-declaration]
 --> libs/wire/point.proto:1:1
  |
1 | syntax = "proto3";
  | ^^^^^^^^^^^^^^^^^
  |
  = this reader implements Protobuf Editions. proto2 and proto3 differ from it
    in field presence, in what a default means on the wire, and in whether an
    enum is open
  = fix: migrate it: `edition = "2026";`, drop every `optional` and `required`
    label, and write `[features.field_presence = IMPLICIT]` on the fields that
    had none
```

Every feature that affects the wire or the JSON resolves identically at editions
2023, 2024 and 2026, because protobuf gives a feature the default of the closest
edition at or before it, and nobody has introduced such a default since 2023.

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

**A singular field is `Option<T>`.** That buys you two different messages for
*absent* and *set to the zero value*, and both survive a round trip:

- `.None` is not written at all. Nothing on the wire, nothing in the JSON.
- `.Some(0)` goes out as two bytes and reads back as `.Some(0)`.

`features.field_presence = IMPLICIT` asks for the old behaviour, on a file, a
message, or a single field. An implicit field is a bare `T`. A value equal to
the type's default is indistinguishable from an absent one, and the encoder
skips the field when it holds that default. That is exactly what a proto3
singular field was, which makes it the migration for one.

A singular *message* field is `Option<T>` whatever the feature says. That is
protobuf's rule rather than this mapping's: there is no "default message" for an
absent one to mean.

Buri refuses `LEGACY_REQUIRED` by name. It describes a proto2 `required` field,
and a field that must be there is a promise the format cannot keep across
versions.

## Names

A field's Buri name is protoc's `json_name`. Drop each `_` and capitalise the
letter after it, and change no other case. `user_name` is `userName` in the
struct *and* in the JSON document, so you have one name to remember.

A field whose name collides with a Buri keyword gets a trailing underscore, so
`type` becomes `type_`. Its JSON name stays as written, because the document is
not ours to rename.

Nested types flatten with an underscore, because Buri has no nested type
namespace. `Everything.Note` is `Everything_Note`, a module-level declaration
beside its parent. A `oneof` named `contact` inside `Everything` becomes
`Everything_Contact` by the same rule.

## Scalars

| proto3 | Buri | |
|---|---|---|
| `int32` `int64` `sint32` `sint64` `uint32` `uint64` | `Int` | |
| `fixed32` `fixed64` `sfixed32` `sfixed64` | `Int` | |
| `double` `float` | `Float` | a `float` field rounds to binary32 on the way out |
| `bool` | `Bool` | |
| `string` | `Str` | UTF-8 on the wire. Bytes that are not text are an error |
| `bytes` | `[U8]` | |

**64-bit fields round-trip on every backend.** An `Int` is an `I64` everywhere,
and on the JavaScript backend an `I64` is a `BigInt`
([`core/number`](../standard-library.md)). So a `uint64` or `int64` field carrying
a value past 2^53 survives with every digit. One thing does hold on every
backend: a `uint64` above 2^63 reads back negative, which is what a signed
reading of those bits gives you.

Negative numbers cost bytes. The encoder writes a negative `int32` or `int64` as
the ten-byte varint of its 64-bit two's complement, exactly as protoc writes
one. A schema whose numbers are often negative should say `sint32`/`sint64`,
which zigzag first: -1 is one byte rather than ten.

## Enums

```proto
enum Shade {
  SHADE_UNSPECIFIED = 0;
  LIGHT = 1;
  DARK = 2;
}
```

becomes a Buri enum whose variants carry the proto value names verbatim:
`Shade.SHADE_UNSPECIFIED`, `Shade.DARK`. Proto3 JSON writes an enum as the
*name* of its value, so renaming them here would make the document say one thing
and the type another.

An open enum's first value must be zero, and this reader requires it too. The
zero value is what an unset field means.

**Editions enums are open, and an open enum keeps what it does not recognise.**
`features.enum_type` defaults to `OPEN`. A value the schema does not name is
part of the *value* rather than an unknown field, so every generated enum
carries one extra variant:

```text
export enum Shade {
  SHADE_UNSPECIFIED,
  LIGHT,
  DARK,
  Unrecognized(Int),
}
```

So a message written by a newer schema survives an older one reading it and
writing it again. In JSON the unrecognised value goes out as its number. If a
schema already has a value called `Unrecognized`, that meaning wins and the
extra variant becomes `Unrecognized_`.

Buri refuses `features.enum_type = CLOSED` by name. A closed enum makes an
unrecognised value an unknown *field*, and a generated struct has nowhere to
keep one.

## `oneof`

```proto
message Everything {
  oneof contact {
    string phone = 60;
    Address office = 61;
  }
}
```

becomes an enum of the cases, held as an `Option`. A `oneof` may be unset, and
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

A oneof tracks presence, so the encoder still writes a case holding its own
type's default. `.Some(.Phone(""))` puts an empty string on the wire and reads
back as itself. `.None` does not.

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

`defaultM` is what makes a message with more than a few fields writable at all.
[The guide](../../guides/proto.md#use-the-types) shows it in use.

`ProtoError` comes from [`core/proto`](../standard-library.md). Every case of it
carries a byte offset or a field number, so a failure is not something you
bisect for.

### Why the codecs are generated Buri

`derive ToJson` walks a *descriptor* the compiler already ships: field names,
variant shapes, element types. A protobuf message is made of field *numbers* and
wire *types*, which that descriptor carries neither of. Generating Buri instead
means the real checker checks the generated codec, the real optimiser optimises
it, dead-code elimination reaches it, and it needs no new intrinsic. What the
schemas *share* lives in `core/proto`: tags, wire types, packed readers, the
error type.

## The wire format

Ordinary proto3. Three things you would otherwise have to check:

- **The encoder writes a field that was set, and skips one that was not.** It is
  what makes `defaultM()`, every field `.None`, encode to zero bytes. An
  `IMPLICIT` field has no "was set", so proto3's rule applies there instead: the
  encoder writes it unless it holds the type's default.
- **A repeated numeric field is packed.** `features.repeated_field_encoding`
  defaults to `PACKED`, and `EXPANDED` asks for one whole field per element. A
  repeated `string`, `bytes`, or message field is never packed, whatever the
  feature says. A reader accepts both forms of the numeric one, because a writer
  may send either.
- **A reader skips a field the schema does not know.** It *drops* the skipped
  bytes rather than keeping them, because a generated type has nowhere to put
  them, so re-encoding a message decoded from a newer schema loses the fields
  that schema added. A known field arriving with a wire type it cannot have gets
  skipped the same way.

A singular field that appears twice in one message takes the last occurrence.
For a scalar that is what the specification says. For a *message* field the
specification asks for a recursive merge instead, and last-wins is the rule a
generated struct with no mutation can express.

Wire types 3 and 4 are proto2's groups, and the reader refuses them rather than
skipping them. A group is a nesting the reader would have to understand to get
past, so treating one as an unknown field would silently read the rest of the
message at the wrong offset.

Fields go out in schema order, so the same value is always the same bytes.

## The JSON mapping

`encodeMJson` writes proto3 JSON, which is **not** what `derive ToJson` writes.
Four differences:

- A 64-bit integer is a **string**. A JSON number is a double, and
  `9007199254740993` is not one. A reader accepts a number as well.
- `bytes` is **base64**, padded, not an array of numbers.
- An enum is the **name** of its value, not a tagged object. A number is
  accepted on the way in, and an unrecognised one is the zero value, exactly as
  in the binary format.
- A `oneof`'s selected case is an **ordinary member** of the enclosing object:
  `{"phone":"9"}`, not `{"contact":{"Phone":"9"}}`. `derive ToJson` would write
  the tagged form, and no other protobuf implementation reads it.

Beyond those, the writer omits a field that was not set and writes one that was,
including at its zero value. It omits an `IMPLICIT` field holding its default,
and omits an empty repeated field rather than writing `[]`. On the way in, an
absent member and a `null` member mean the same thing, and the reader ignores a
member the schema does not know.

One deviation from the specification, recorded rather than hidden. The writer
puts members in schema order with a `oneof`'s case last, rather than strictly in
field-number order. JSON objects are unordered, so no conforming reader can
notice.

A failure names the path it happened at, written the way
[`core/json`](../standard-library.md) writes one: `$` for the document, `.name`
for a member. So `$.home.city` is a place a reader can find in the text in front
of them.

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

**There is no unpacking.** `value` holds the encoded bytes of some other
message, and only a runtime type registry could say which message the
`type_url` names. This toolchain has no such registry. A program that wants what
is inside an `Any` decodes `value` itself, with the codec for the type it
expects:

```text
let inner = decodeErrorInfo(ctx, detail.value)?;
```

Two consequences:

- **The JSON is the two-field object**, `{"typeUrl": "…", "value": "…"}`, with
  the bytes base64 as any `bytes` field is. Canonical `Any` JSON inlines the
  held message and adds an `@type` member, which needs the same registry. Any
  implementation reads a document written here as an ordinary message, and one
  expecting the canonical form does *not* read it as an `Any`.
- **Nothing checks the `type_url`.** Whether the bytes match the URL is between
  the two programs exchanging them.

The binary format is exact. An `Any` written here is an `Any` everywhere,
because the wire encoding of the message is its two fields and always was.

## What is not supported

Buri refuses each of these by name, with the reason and the edit, under
`proto-unsupported`:

| | Why not |
|---|---|
| `service`, `rpc` | This reader turns a schema into data types. There is no RPC transport to generate a stub against. |
| `extend`, `extensions` | An extension adds fields to a message from outside it, so the generated type would not be the whole of the message. |
| `group` | proto2's inline nesting, whose wire encoding was removed from proto3. Declare a nested `message`. |
| `map<K, V>` | Sugar for a repeated entry message with its own wire layout, and Buri's `Map` is not ordered the way a decoded map would have to be. Declare the entry message. |
| the `optional` and `required` labels | Editions removed both; presence is `features.field_presence` now. protoc refuses them in the same words. |
| `import public` | Re-exports another file's declarations, which would make one module's surface depend on a second file's. |
| `syntax = "proto2"`, `syntax = "proto3"`, editions before 2026 | See [Editions, and only one](#editions-and-only-one). |
| `features.field_presence = LEGACY_REQUIRED` | A field that must be there is a promise the format cannot keep across versions. |
| `features.enum_type = CLOSED` | An unrecognised value would become an unknown field, which a generated struct has nowhere to keep. |
| `features.message_encoding = DELIMITED` | The group encoding again, under its new name. |
| `features.utf8_validation = NONE` | A `string` field becomes a `Str`, and a `Str` is text. Declare the field `bytes`. |
| `features.json_format = LEGACY_BEST_EFFORT` | It describes what proto2 did to JSON. This writes the one mapping editions defines. |
| `option features = { ... }` | The block form of a feature. One spelling of a thing is enough. |

`features.enforce_naming_style` and `features.default_symbol_visibility` are
source-retention lints. They say nothing about what a message means, so the
reader reads past them rather than refusing. That is what lets a schema opt out
of the naming style protoc enforces from edition 2024 on.

`option` and `reserved` are the two statements the reader *skips* rather than
refuses. Neither says anything about the shape of a message.

You write an `import` inside a schema from the repository root, the way protoc
resolves one against `-I.`:

```proto
import "lib/proto/address.proto";
```

## Is it right?

Protobuf ships a conformance suite, a C++ runner that drives a few thousand
wire-format and JSON edge cases at an implementation. Buri is onboarded to it.
`cli/tests/proto/` holds the vendored schemas, a testee that is a Buri binary,
and a failure list filing every expected failure under a reason.

```text
CONFORMANCE SUITE PASSED: 970 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

[`cli/tests/proto/README.md`](../../../../tests/proto/README.md) files every
expected failure under one of seven reasons. One of the seven is not a gap: the
reference implementation is proto3 and the schema under test is edition 2026, so
the two disagree about whether a writer writes a field set to its zero value.
Both are right about their own schema.

The conformance run is not part of `cargo test`, because a suite that needs a
C++ build of another project is a suite that does not run.
`cli/tests/vectors/proto.rs` replays recorded exchanges through the same testee
under cargo.

## Caching

A schema is an input like any other. Its contents go into the declaring rule's
key, so editing one rebuilds exactly what depends on it and nothing else.
`--explain` reports a `generate` action per rule that declares a generator:

```text
keyed  generate //lib/wire js a47062e1d851
keyed  compile //lib/wire js 13a53a25987e
run    link //cmd/app js 1c42f9658fa5
```

The action is `keyed` rather than `cached` for the same reason `compile` is.
This toolchain caches a binary's whole closure under one `link` key, so the
generated module has a key and no cache entry of its own.
