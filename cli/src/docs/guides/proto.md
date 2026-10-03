# Import a `.proto` schema

A `.proto` file in a package becomes a module. Nothing is written to your
source tree, so there's no `_pb.buri` to check in.

## Put the schema in the package

The schema must be edition 2026. The compiler refuses proto2, proto3 and older
editions.

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

Write the `generators` entry yourself; `buri gen` can't know which generator
owns a file:

```textproto schema=build
# libs/wire/BUILD.buri
library {
    generators: [
        { tool: "proto", inputs: ["point.proto"] },
    ]
    visibility: ["//visibility:public"]
}
```

A schema no entry lists is `unused-source`.

## Decide what leaves the library

A schema exports everything it declares, and `lib.buri` picks what leaves the
library:

```text
// libs/wire/lib.buri
from "//libs/wire/point.proto" export {
    decodePoint, decodePointJson, defaultPoint, encodePoint, encodePointJson, Point,
};
```

The import path is the schema's own path, extension included.

## Use the types

Each message brings a default, a binary codec and a JSON codec: for `Point`,
`defaultPoint`, `encodePoint`/`decodePoint` and
`encodePointJson`/`decodePointJson`. The codecs allocate, so they take a
context:

```buri repo=cli/tests/conformance package=//lib/proto
from "core/proto" import { ProtoError };
from "platform/effect" import { Allocator };
from "//lib/proto/address.proto" import { Address, decodeAddress, encodeAddress };

export fn roundTrip<C: Allocator>(ctx: C, a: Address): Result<Address, ProtoError> {
    decodeAddress(ctx, encodeAddress(ctx, a))
}
```

**Every singular field is an `Option`**, since editions track presence.
`.Some(...)` and `.None` are different messages on the wire. Start from
`default...()` and set only what you need:

```buri repo=cli/tests/conformance package=//lib/proto
from "//lib/proto/demo.proto" import { defaultEverything, Everything, Shade };

export fn dark(): Everything {
    Everything { ..defaultEverything(), name: .Some("Ada"), shade: .Some(Shade.DARK) }
}
```

A failure is a `ProtoError` carrying a byte offset or a field number.

## Check and format it

`buri build`, `buri test`, `buri lint` and your editor check a schema before
generating from it:

```text
error: `radius` and `sides` both use field number 3 [proto-duplicate-field]
  --> libs/wire/shape.proto:17:5
   |
17 |     double radius = 3;
   |     ^^^^^^^^^^^^^^^^^^
```

`buri format` lays the schema out, and `--check` fails when it would change:

```proto
// libs/wire/point.proto
edition = "2026";

package demo.v1;

message Point {
    int32 x = 1; // east
    int32 y = 2;
}
```

## Share a schema between packages

Depend on the library and use what its `lib.buri` re-exports. A schema may
`import` another only from the same rule.

## Write a message's values

A `.txtpb` file holds one message value in protobuf's text format, checked
against the schema: see [check text format files against a
message](./textproto.md).

---

[`proto.md`](../reference/build/proto.md) is the full mapping, including the
wire and JSON formats and what's refused.
