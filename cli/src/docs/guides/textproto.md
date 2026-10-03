# Check text format files against a message

A `.txtpb` file holds one protobuf message value. List it in a generator's
`inputs`, and the build checks it against its message:

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

A mistake stops the generator:

```text
error: `ports` is of type `int32`, and this is not a whole number [textproto-wrong-value]
 --> lib/deploy/server.txtpb:5:13
  |
5 | ports: [80, "443"]
  |             ^^^^^
  |
  = fix: write a whole number
```

`buri build`, `buri test`, `buri lint` and your editor run the check.

## Which files

Files a rule's `inputs` lists, ending `.txtpb` or `.textproto`. `REPO.buri`
can add extensions:

```textproto schema=repo
language {
    name: "textproto"
    extensions: [".pbtxt"]
}
```

## The header

- **It's required.** `# proto-file:` and `# proto-message:` go in comments
  above the first field, per the
  [text format specification](https://protobuf.dev/reference/protobuf/textformat-spec/#header).
  Without them it's
  [`textproto-without-header`](../reference/errors/textproto-without-header.md).
- **`proto-file`** is relative to the file, or a `//` path. A URL or a path
  outside the repository is
  [`schema-not-local`](../reference/errors/schema-not-local.md).
- **`proto-message`** is relative to the schema's `package`, or fully
  qualified, as `deploy.v1.Server`.
- `# proto-import:` is refused; the schema's own `import`s bring in what it
  uses.

A file read by a tool with a
[contract](../reference/build/tools.md#input-contracts) may omit the header; if
present, it must name the contract's schema and message.

## What the check holds a file to

- every field is one the message declares, by its schema name;
- every value is its field's type: a quoted string for `string` and `bytes`, a
  whole number in range for the integers, a number or `inf` or `nan` for
  `float` and `double`, `true` or `false` for `bool`, a value's name or a
  number for an enum, and `{ ... }` for a message;
- a field that is not `repeated` is set once, and takes no list;
- a `oneof` holds one of its cases.

No field is required: a missing field is unset, or zero where
`features.field_presence = IMPLICIT`. The schema is checked as
[`proto`](./proto.md) checks it.

An extension or `Any` field (`[name]`) is
[`textproto-unsupported`](../reference/errors/textproto-unsupported.md).

## Formatting

`buri format` writes one field per line, at `.buri` width and indent, and
keeps every comment:

```textproto ignore why="a data file, not a build file"
name: "api"
ports: [80, 443]
limits {
    cpu: 0.5
}
stops: [{ city: "Springfield" }, { city: "Shelbyville" }]
```

A scalar takes `:`, a message takes `{ }`, and `< >` becomes `{ }`. Trailing
`;` or `,` goes. Lists stay on one line when they fit. Literals keep their
spelling, so the value never changes. A file that doesn't parse is left alone,
and `buri format --check` names it.

## Generating its value

`textproto` in `generators` gives a module named after the file, holding the
message's types and the file's value:

```buri ignore why="it imports a module the build generates from the text format file"
from "//lib/deploy/server.txtpb" import { Server, server };
```

The value is an `export let` named after the file up to its first `.`, in
camel case: `server: Server`. The types are what
[`proto`](../reference/build/proto.md) generates. A type the schema imports
comes from that file's own module, so list that schema under `proto` too.

A tool with a `textproto` [contract](../reference/build/tools.md#input-contracts)
gets the same types and each file as a typed value.
