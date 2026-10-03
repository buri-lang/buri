# Importing a `.proto` schema

A `.proto` file in a package is a source: the compiler generates its module, and
nothing lands in the source tree. This page is the mapping, which is what another
program will find in the bytes. For the task, read [import a `.proto`
schema](../../guides/proto.md).

The schema must be edition 2026. Buri refuses `syntax = "proto3"`, proto2, and
older editions (see [Editions, and only one](#editions-and-only-one)).

The import path is the schema's own path, extension included:
`//lib/proto/address.proto` names `lib/proto/address.proto` on disk.

## Declaring the schema

```textproto schema=build
library {
    generators: [
        { tool: "proto", inputs: ["address.proto", "demo.proto"] },
    ]
}
```

You write `generators` by hand; `buri gen` can't know which generator owns a
file. A schema no entry lists is
[`unused-library`](../lints/unused-library.md). The old `proto_sources` field is
[retired](../errors/retired-proto-sources.md).

The generated module belongs to the declaring rule, so library boundaries apply.
`//lib/wire/point.proto` is internal to `//lib/wire`; other packages reach its
types through `lib.buri` re-exporting them. A schema exports everything it
declares, and `lib.buri` picks which names leave the library.

A schema may `import` another only from the same rule. To share a schema across
packages, re-export its types from the owning library's `lib.buri`.

`unused-import` and `dead-code` skip generated modules, since there's no file to
edit.

## Checking

`proto` is a built-in language, like `json`. `buri build`, `buri test`,
`buri lint` and the language server check every `.proto` in a rule's `inputs`
before any generator reads it. A `REPO.buri` may give it more extensions and
nothing else.

The check reports everything `generate` would, each on its own span:

- a statement that does not parse ([`proto-schema`](../errors/proto-schema.md));
- the edition, and everything the reader refuses (see below);
- a type that names nothing, or two things
  ([`proto-unknown-type`](../errors/proto-unknown-type.md),
  [`proto-ambiguous-type`](../errors/proto-ambiguous-type.md));
- a field number or name used twice in a message, `oneof` cases included, or
  one the message `reserved`
  ([`proto-field-reused`](../errors/proto-field-reused.md));
- an `import` that names no schema in the repository
  ([`proto-import-not-found`](../errors/proto-import-not-found.md)). Imports
  resolve from the repository root, never outside it.

A schema that fails never reaches its generator.

## Formatting

`buri format` and your editor lay out every `.proto` in a rule's `inputs`. There
are no options:

```proto
// before
message Point{int32 x=1;int32 y=2 [deprecated=true];}

// after
message Point {
    int32 x = 1;
    int32 y = 2 [deprecated = true];
}
```

- One statement per line, four spaces of indent per block.
- Blank lines between statements collapse to one.
- A field's `[...]` options break one per line past the margin.
- Every comment stays where it was: on its own line, or at the end of its
  statement's line.
- An unclosed string, comment or bracket leaves the file untouched.

The formatter moves only whitespace and comments, so meaning never changes.

## Editions, and only one

A schema declares `edition = "2026";`. Buri refuses everything else, including a
file that declares nothing.

Editions changed what a singular field means: under proto3 a singular scalar has
no presence, under editions it does by default. No reader can paper over that,
so Buri refuses an older file and puts the migration in the `fix`:

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

Every feature affecting the wire or JSON resolves the same at editions 2023,
2024 and 2026, so moving a 2023 schema to 2026 changes no bytes.

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

derive Equal, Show for Person;
```

| editions | Buri |
|---|---|
| `message` | `struct` with named fields, `derive Equal, Show` |
| singular `T` (the default, EXPLICIT presence) | `Option<T>` |
| singular `T` with `features.field_presence = IMPLICIT` | `T` |
| singular message field | `Option<T>`, whatever the feature says |
| `repeated T` | `[T]` |
| `oneof pick { ... }` | `enum Person_Pick`, held as `Option<Person_Pick>` |
| `message Outer { message Inner { } }` | `Outer` and `Outer_Inner`, side by side |
| `enum Colour` | `enum Colour`, value names verbatim, plus `Unrecognized(Int)` |

### Presence

A singular field is `Option<T>`, so *absent* and *set to zero* stay different
through a round trip:

- `.None` writes nothing, on the wire or in JSON.
- `.Some(0)` writes two bytes and reads back as `.Some(0)`.

`features.field_presence = IMPLICIT`, on a file, message or field, gives you a
bare `T`. A value equal to the type's default is then indistinguishable from
absent, and the encoder skips it. That's a proto3 singular field, which makes it
the migration for one.

A singular *message* field is always `Option<T>`: protobuf has no "default
message" for an absent one to mean.

Buri refuses `LEGACY_REQUIRED`. A field that must be present is a promise the
format can't keep across versions.

## Names

A field's Buri name is protoc's `json_name`: drop each `_` and capitalise the
letter after it. `user_name` is `userName` in the struct *and* the JSON.

A name that's a Buri keyword gets a trailing underscore (`type` becomes `type_`),
but its JSON name stays as written.

Nested types flatten with an underscore, since Buri has no nested namespaces.
`Everything.Note` is `Everything_Note`, and a `oneof contact` inside
`Everything` is `Everything_Contact`.

## Scalars

| proto3 | Buri | |
|---|---|---|
| `int32` `int64` `sint32` `sint64` `uint32` `uint64` | `Int` | |
| `fixed32` `fixed64` `sfixed32` `sfixed64` | `Int` | |
| `double` `float` | `Float` | a `float` field rounds to binary32 on the way out |
| `bool` | `Bool` | |
| `string` | `Str` | UTF-8 on the wire. Bytes that are not text are an error |
| `bytes` | `[U8]` | |

64-bit fields round-trip on every backend. An `Int` is an `I64` everywhere, and a
`BigInt` on JavaScript ([`core/number`](../standard-library.md)), so values past
2^53 keep every digit. A `uint64` above 2^63 reads back negative.

The encoder writes a negative `int32` or `int64` as a ten-byte varint, like
protoc. Use `sint32`/`sint64` for numbers that are often negative: they zigzag
first, so -1 is one byte.

## Enums

```proto
enum Shade {
  SHADE_UNSPECIFIED = 0;
  LIGHT = 1;
  DARK = 2;
}
```

becomes a Buri enum with the value names verbatim: `Shade.SHADE_UNSPECIFIED`,
`Shade.DARK`. Proto3 JSON writes an enum as its value's name, so renaming would
make the document and the type disagree.

The first value must be zero; it's what an unset field means.

Editions enums are open (`features.enum_type` defaults to `OPEN`), so an unknown
value is kept as part of the value. Every generated enum carries one extra
variant:

```text
export enum Shade {
  SHADE_UNSPECIFIED,
  LIGHT,
  DARK,
  Unrecognized(Int),
}
```

A message from a newer schema survives an older one reading and rewriting it. In
JSON an unrecognised value goes out as its number. If the schema already has an
`Unrecognized` value, the extra variant becomes `Unrecognized_`.

Buri refuses `features.enum_type = CLOSED`: it makes an unknown value an unknown
*field*, which a generated struct can't keep.

## `oneof`

```proto
message Everything {
  oneof contact {
    string phone = 60;
    Address office = 61;
  }
}
```

becomes an enum of the cases, held as an `Option` because a `oneof` may be
unset:

```text
export enum Everything_Contact {
  Phone(Str),
  Office(Address),
}

export struct Everything {
  export contact: Option<Everything_Contact>,
}
```

A `oneof` tracks presence: `.Some(.Phone(""))` writes an empty string and reads
back as itself. `.None` writes nothing.

## What comes with each type

For a message `M`, all exported:

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

`defaultM` makes a message with many fields writable;
[the guide](../../guides/proto.md#use-the-types) shows it in use.

`ProtoError` comes from [`core/proto`](../standard-library.md). Every case
carries a byte offset or a field number.

### Why the codecs are generated Buri

`derive ToJson` walks a descriptor of field names and variant shapes, but
protobuf needs field numbers and wire types, which it lacks. Generated Buri gets
checked, optimised and dead-code eliminated like any other code, and needs no new
intrinsic. Shared pieces (tags, wire types, packed readers, the error type) live
in `core/proto`.

## The wire format

Ordinary proto3, plus:

- **The encoder writes a field that was set and skips one that wasn't**, so
  `defaultM()` encodes to zero bytes. An `IMPLICIT` field is written unless it
  holds the type's default.
- **A repeated numeric field is packed.** `features.repeated_field_encoding`
  defaults to `PACKED`; `EXPANDED` writes one field per element. Repeated
  `string`, `bytes` and message fields are never packed. Readers accept both
  forms.
- **A reader drops fields the schema doesn't know**, because a generated type has
  nowhere to keep them. Re-encoding a message from a newer schema loses the fields
  it added. A known field with an impossible wire type is dropped the same way.

A singular field that appears twice takes the last occurrence. For a message
field the spec asks for a recursive merge; last-wins is what an immutable struct
can express.

The reader refuses wire types 3 and 4 (proto2 groups). Skipping one would read
the rest of the message at the wrong offset.

Fields go out in schema order, so the same value is always the same bytes.

## The JSON mapping

`encodeMJson` writes proto3 JSON, which differs from `derive ToJson`:

- A 64-bit integer is a **string**, since `9007199254740993` isn't a double.
  Readers accept a number too.
- `bytes` is padded **base64**.
- An enum is its value's **name**. Readers accept a number, and an unrecognised
  one is the zero value, as in the binary format.
- A `oneof`'s case is an **ordinary member** of the enclosing object:
  `{"phone":"9"}`, not `{"contact":{"Phone":"9"}}`.

The writer omits unset fields, `IMPLICIT` fields at their default, and empty
repeated fields; it writes set fields even at zero. The reader treats `null` as
absent and ignores unknown members.

One deviation from the spec: members go out in schema order with a `oneof`'s
case last, not in field-number order. JSON objects are unordered, so no
conforming reader can tell.

A failure names its path the way [`core/json`](../standard-library.md) does:
`$.home.city`.

## `google.protobuf.Any`, as its two fields

Vendor the schema
[googleapis publishes](https://github.com/protocolbuffers/protobuf/blob/main/src/google/protobuf/any.proto),
declare it in a rule, and an `Any` field reads as a plain struct:

```text
export struct Any {
  export typeUrl: Str,
  export value: [U8],
}
```

There's no unpacking, because that needs a runtime type registry and Buri has
none. Decode `value` with the codec for the type you expect:

```text
let inner = decodeErrorInfo(ctx, detail.value)?;
```

- **The JSON is the two-field object**, `{"typeUrl": "…", "value": "…"}`, with
  `value` in base64. Canonical `Any` JSON inlines the message with an `@type`
  member, which needs the registry, so a reader expecting that form won't read
  this as an `Any`.
- **Nothing checks `type_url`** against the bytes.

The binary format is exact: an `Any` written here is an `Any` everywhere.

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

The reader ignores `features.enforce_naming_style` and
`features.default_symbol_visibility`, which are source-only lints. So a schema
can opt out of protoc's edition-2024 naming style.

The reader skips other `option` statements, and reads `reserved` only to check
no field uses it.

Write an `import` from the repository root, as protoc resolves one against `-I.`:

```proto
import "lib/proto/address.proto";
```

## Is it right?

Buri runs protobuf's conformance suite, a C++ runner with a few thousand
wire-format and JSON edge cases. `cli/tests/proto/` holds the vendored schemas,
a Buri testee, and a list of expected failures.

```text
CONFORMANCE SUITE PASSED: 970 successes, 1314 skipped, 456 expected failures, 0 unexpected failures.
```

[`cli/tests/proto/README.md`](../../../../tests/proto/README.md) files each
expected failure under one of seven reasons. One isn't a gap: the reference
implementation is proto3 and the test schema is edition 2026, so they disagree
about writing a field set to zero. Both are right for their own schema.

The suite needs a C++ build of another project, so it isn't part of
`cargo test`. `cli/tests/vectors/proto.rs` replays recorded exchanges through the
same testee under cargo.

## Caching

A schema's contents go into the declaring rule's key, so editing one rebuilds
only what depends on it. `--explain` shows a `generate` action per rule with a
generator:

```text
keyed  generate //lib/wire js a47062e1d851
keyed  compile //lib/wire js 13a53a25987e
run    link //cmd/app js 1c42f9658fa5
```

It's `keyed` rather than `cached`, like `compile`: a binary's whole closure is
cached under one `link` key, so the generated module has no cache entry of its
own.
