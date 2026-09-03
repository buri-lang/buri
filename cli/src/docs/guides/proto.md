# Import a `.proto` schema

A `.proto` file in a package becomes a module. Nothing is written to the source
tree: there is no `_pb.buri` to check in and no generation step to forget.

## Put the schema in the package

It must be an edition-2026 schema. `syntax = "proto3"` is refused, and so are
proto2 and the older editions.

```proto
// libs/wire/point.proto
edition = "2026";

package demo.v1;

message Point {
  int32 x = 1;
  int32 y = 2;
}
```

## Declare it

A schema belongs to a rule the way a `.buri` does. `buri gen` writes the field
for you:

```text
$ buri gen
updated libs/wire/BUILD.buri
  + proto_sources: point.proto
```

```textproto schema=build
# libs/wire/BUILD.buri
library {
    proto_sources: ["point.proto"]
    visibility: ["//visibility:public"]
}
```

A schema no rule lists is `unused-library`, the same error a stray `.buri` gets.

## Decide what leaves the library

A schema exports everything it declares — that is what a schema *is* — and the
library boundary applies to the generated module unchanged. `lib.buri` picks:

```text
// libs/wire/lib.buri
from "//libs/wire/point.proto" export {
    decodePoint, decodePointJson, defaultPoint, encodePoint, encodePointJson, Point,
};
```

The import path is the schema's own path, extension included. There is one
spelling and it is the file's name.

## Use the types

Each message brings a default, a binary codec, and a JSON codec: for `Point`
those are `defaultPoint`, `encodePoint`/`decodePoint`, and
`encodePointJson`/`decodePointJson`. Encoding and decoding allocate, so they
take a context — here for an `Address` message in another repository:

```buri repo=cli/tests/conformance package=//lib/proto
from "core/effect" import { Alloc };
from "core/proto" import { ProtoError };
from "//lib/proto/address.proto" import { Address, decodeAddress, encodeAddress };

export fn roundTrip<C: Alloc>(ctx: C, a: Address): Result<Address, ProtoError> {
    decodeAddress(ctx, encodeAddress(ctx, a))
}
```

**Every singular field is an `Option`**, because presence is the edition's
default. Setting one is `.Some(...)`, leaving it out is `.None`, and the two are
different messages on the wire. The default is what makes a message with more
than a few fields writable:

```buri repo=cli/tests/conformance package=//lib/proto
from "//lib/proto/demo.proto" import { defaultEverything, Everything, Shade };

export fn dark(): Everything {
    Everything { ..defaultEverything(), name: .Some("Ada"), shade: .Some(Shade.DARK) }
}
```

A failure is a `ProtoError` carrying a byte offset or a field number, so a
malformed four-kilobyte message says where it went wrong.

## Share a schema between packages

Depend on the library and use what its `lib.buri` re-exported — a generated
module is inside the boundary like any other. One schema may `import` another,
and when it does both must belong to the same rule.

---

[`proto.md`](../reference/build/proto.md) is the mapping: what each proto
construct becomes, what the wire and JSON formats are, and which constructs are
refused.
