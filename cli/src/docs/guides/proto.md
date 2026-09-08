# Import a `.proto` schema

A `.proto` file in a package becomes a module. The compiler writes nothing to
your source tree: no `_pb.buri` to check in, no generation step to forget.

## Put the schema in the package

The schema must be edition 2026. The compiler refuses `syntax = "proto3"`, and
proto2 and the older editions with it.

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

A schema is a generator's input, and `std/codegen/proto` is the generator. Write
the entry yourself — `buri gen` cannot know which generator owns a file, so it
leaves `generators` alone:

```textproto schema=build
# libs/wire/BUILD.buri
library {
    generators: [
        {
            tool: "std/codegen/proto"
            inputs: ["point.proto"]
        }
    ]
    visibility: ["//visibility:public"]
}
```

A schema no entry lists is `unused-library`, the same finding a stray `.buri`
gets.

## Decide what leaves the library

A schema exports everything it declares. The library boundary applies to the
generated module unchanged, so `lib.buri` picks:

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
`encodePointJson`/`decodePointJson`. Encoding and decoding allocate, so they
take a context — here for an `Address` message in another repository:

```buri repo=cli/tests/conformance package=//lib/proto
from "core/effect" import { Allocator };
from "core/proto" import { ProtoError };
from "//lib/proto/address.proto" import { Address, decodeAddress, encodeAddress };

export fn roundTrip<C: Allocator>(ctx: C, a: Address): Result<Address, ProtoError> {
    decodeAddress(ctx, encodeAddress(ctx, a))
}
```

**Every singular field is an `Option`**, because presence is the edition's
default. Setting one is `.Some(...)`, leaving it out is `.None`, and the two are
different messages on the wire. `default...()` with an update is what makes a
message of more than a few fields writable:

```buri repo=cli/tests/conformance package=//lib/proto
from "//lib/proto/demo.proto" import { defaultEverything, Everything, Shade };

export fn dark(): Everything {
    Everything { ..defaultEverything(), name: .Some("Ada"), shade: .Some(Shade.DARK) }
}
```

A failure is a `ProtoError` carrying a byte offset or a field number, so a
malformed message says where it went wrong.

## Share a schema between packages

Depend on the library and use what its `lib.buri` re-exported. One schema may
`import` another, and then both must belong to the same rule.

---

[`proto.md`](../reference/build/proto.md) is the mapping: what each proto
construct becomes, what the wire and JSON formats are, and which constructs are
refused.
