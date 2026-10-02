# Check text format files against a message

A `.txtpb` file holds one value of a protobuf message. List it in a generator's
`inputs`, and the build checks it against its message before the generator
reads it:

```textproto schema=build
library {
    generators: [
        { tool: "textproto", inputs: ["server.txtpb"] },
    ]
}
```

```textproto ignore why="a data file, not a build file"
# proto-file: server.proto
# proto-message: Server

name: "api"
ports: [80, 443]
limits {
    cpu: 0.5
}
```

```proto
// lib/deploy/server.proto
edition = "2026";

package deploy.v1;

message Server {
    string name = 1;
    repeated int32 ports = 2;
    Limits limits = 3;

    message Limits {
        float cpu = 1;
    }
}
```

A mistake is reported where it is, and the generator does not run:

```text
error: `ports` is of type `int32`, and this is not a whole number [textproto-wrong-value]
 --> lib/deploy/server.txtpb:5:13
  |
5 | ports: [80, "443"]
  |             ^^^^^
  |
  = fix: write a whole number
```

`buri build`, `buri test` and `buri lint` run the check, `buri format` lays the
file out, and your editor checks it as you type.

## Which files

Only a file some rule's `inputs` lists, ending `.txtpb` or `.textproto`.
`REPO.buri` can add extensions:

```textproto schema=repo
language {
    name: "textproto"
    extensions: [".pbtxt"]
}
```

## The header

- **It is required.** `# proto-file:` and `# proto-message:` go in the comments
  above the first field, as the
  [text format specification](https://protobuf.dev/reference/protobuf/textformat-spec/#header)
  writes them. A file without them is
  [`textproto-without-header`](../reference/errors/textproto-without-header.md).
- **The schema is checked in.** `proto-file` is a path relative to the file, or
  a `//` path. A URL, or a path that leaves the repository, is
  [`schema-not-local`](../reference/errors/schema-not-local.md).
- **The message is the schema's.** `proto-message` is a name relative to the
  schema's `package`, or the whole name, as `deploy.v1.Server`.
- `# proto-import:` is refused: the specification gives it no meaning, and the
  schema's own `import`s bring in what it uses.

A file read by a tool with a
[contract](../reference/build/tools.md#input-contracts) may leave the header
out. If it keeps one, it names the contract's schema and message.

## What the check holds a file to

- every field is one the message declares, by its schema name;
- every value is its field's type: a quoted string for `string` and `bytes`, a
  whole number in range for the integers, a number or `inf` or `nan` for
  `float` and `double`, `true` or `false` for `bool`, a value's name or a
  number for an enum, and `{ ... }` for a message;
- a field that is not `repeated` is set once, and takes no list;
- a `oneof` holds one of its cases.

Under edition 2026 no field is required: a missing field is unset, or its zero
value where `features.field_presence = IMPLICIT`. The schema is checked too,
the way [`proto`](./proto.md) checks it.

An extension or `Any` field, written `[name]`, is refused as
[`textproto-unsupported`](../reference/errors/textproto-unsupported.md).

## Formatting

`buri format` writes one field per line at the width and indent every `.buri`
file gets, and keeps every comment:

```textproto ignore why="a data file, not a build file"
name: "api"
ports: [80, 443]
limits {
    cpu: 0.5
}
stops: [{ city: "Springfield" }, { city: "Shelbyville" }]
```

A scalar takes `:`, a message takes `{ }`, and `< >` becomes `{ }`. The `;` or
`,` after a field goes. A list, and a message inside one, stays on a line when
it fits. Strings, numbers and words keep their spelling, so the value never
changes. A file that does not parse is left as it is, and
`buri format --check` names it.

## Generating its value

`textproto` in `generators` gives a module named after the file, holding
the message's types and the file's value:

```buri ignore why="it imports a module the build generates from the text format file"
from "//lib/deploy/server.txtpb" import { Server, server };
```

The value is an `export let` named after the file up to its first `.`, in
camel case: `server: Server`. The types are the ones
[`proto`](../reference/build/proto.md) generates from the schema. A type
the schema imports from another file comes from that file's own module, so list
that schema under `proto` too.

A tool with a `textproto` [contract](../reference/build/tools.md#input-contracts)
gets the same types, generated into the tool, and each file as a typed value.
