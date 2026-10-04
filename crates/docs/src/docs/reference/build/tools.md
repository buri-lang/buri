# Tools

A tool is a program the build runs on a language's files. It's a rule of its
own, a peer of `library` and `binary`:

```textproto schema=build
# tool/lines/BUILD.buri
tool {
    sources: ["words.buri"]

    check {}
    format {}
}
```

A `tool` rule must live under the top-level `tool/` directory, at any depth
(`//tool/db/schema`), or it's
[`misplaced-rule`](../errors/misplaced-rule.md).
Libraries and binaries may live there too.

Its root is `tool.buri`, which exports one function per block:

```buri
from "core/format" import { Doc };
from "core/tool" import { Checked, CheckRequest, Diagnostic, FormatRequest };

// tool/lines/tool.buri
from "platform/effect" import { Allocator };

export fn check<C: Allocator>(ctx: C, request: CheckRequest<Str>): Checked {
    Checked { diagnostics: [], needs: [] }
}

export fn format<C: Allocator>(
    ctx: C,
    request: FormatRequest,
): Result<Doc, [Diagnostic]> {
    .Ok(.Text(request.input.value.trim()))
}
```

A `generate {}` block adds the third:

```buri
from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    Generated { modules: [], diagnostics: [], needs: [] }
}
```

- **Blocks and exports match.** A block without its function is
  [`tool-entry-point-not-exported`](../errors/tool-entry-point-not-exported.md),
  and an exported `check`, `format` or `generate` without its block is
  [`tool-missing-block`](../errors/tool-missing-block.md).
- **`ctx` has only `Allocator`.** Any other effect is
  [`tool-effect-unavailable`](../errors/tool-effect-unavailable.md),
  so a tool's answer depends only on what it was handed.
- **There is no `main`.** The toolchain writes one that reads the request,
  calls the entry point and writes the answer.
- `sources`, `dependencies` and `test` mean what they mean on a `binary`. A
  tool's own tests import `//tool/lines/tool.buri`; nothing else may import a
  tool's modules ([`tool-source-import`](../errors/tool-source-import.md)).
- `buri gen` leaves a package with a `tool` rule as written.

## Who calls which entry point

| Entry point | Called for |
| --- | --- |
| `check` | Each file a rule's `inputs` lists, in a language whose `check` names the tool. Before any generator reads it. |
| `format` | The same files, in a language whose `format` names the tool, by `buri format` and your editor. |
| `generate` | Each `generators` entry naming the tool. See [`generators.md`](./generators.md). |

A language names its tools in [`REPO.buri`](./repo-config.md). Naming a tool
that lacks the entry point is
[`tool-missing-entry-point`](../errors/tool-missing-entry-point.md), and naming
no tool at all is [`unknown-tool`](../errors/unknown-tool.md).

The toolchain ships three tools, each with `check`, `format` and `generate`:
`json` for `json`, `jsonc` and `json5`, `proto` for `.proto` schemas, and
`textproto` for [text format files](../../guides/textproto.md). Built-in tools
have bare names and yours are `//label`s, so they never collide.

## What an entry point is handed

`buri docs core/tool` prints the types. Each input is an `Input<Str>` with its
repository `path`, its `language`, and its text as `value`. Under a
[contract](#input-contracts) it's an `Input<Root>` instead.

- **`check`** answers `Checked`: diagnostics, and the repository paths it
  `needs`. A needed file, such as a schema, comes back in `files` on the next
  call. A path outside the repository is
  [`schema-outside-repository`](../errors/schema-outside-repository.md).
- **`format`** answers a `core/format` `Doc`, laid out at the same margin and
  indent as `.buri` files. A file the tool can't read gets diagnostics and no
  doc, and stays as it is.
- **`generate`** answers modules, diagnostics and `needs`; see
  [`generators.md`](./generators.md).

A diagnostic prints under its `code`'s catalogue page if there is one, else as
[`tool-diagnostic`](../errors/tool-diagnostic.md). A tool that doesn't build or
doesn't answer is [`tool-failed`](../errors/tool-failed.md).

## Input contracts

A tool whose inputs follow one schema declares it:

```textproto schema=build
# tool/database_schema_codegen/BUILD.buri
tool {
    sources: ["emit.buri"]

    generate {
        accepts: [
            { language: "json", type_schema: "config.schema.json" },
        ]
    }
}
```

The consumer lists its file as usual, and the file may leave out `"$schema"`:

```textproto schema=build
# lib/orders/BUILD.buri
library {
    generators: [
        { tool: "//tool/database_schema_codegen", inputs: ["schema.json"] },
    ]
}
```

The build generates the schema's types into the tool as the module
`<tool label>/<language>`, and `generate` takes them:

```buri ignore why="it imports the module the build generates into the tool from its contract"
from "core/tool" import { Generated, GenerateRequest };

// tool/database_schema_codegen/tool.buri
from "platform/effect" import { Allocator };
from "//tool/database_schema_codegen/json" import { Config };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Config>): Generated {
    // request.inputs[i].value is a Config
    Generated { modules: [], diagnostics: [], needs: [] }
}
```

- **The check uses the contract's schema.** The file's own `"$schema"` may be
  absent or name the same schema. Anything else is
  [`schema-mismatch`](../errors/schema-mismatch.md), as is one file read by two
  tools with different contracts.
- **`type_schema` depends on the language.** For `json`, `jsonc` and `json5`
  it's a JSON Schema path relative to the tool's package, or a `//` path
  ([type mapping](../../guides/json.md#generating-types)). For `textproto` it's
  a schema and a message, `routes.proto:Routes`, and a file under it may omit
  its `# proto-file:` and `# proto-message:` header or name the same ones.
  `proto` takes no contract, because a `.proto` file holds no value
  ([`proto-contract-unsupported`](../errors/proto-contract-unsupported.md)).
- **One entry per language.** An input in a language no entry lists is
  [`input-not-accepted`](../errors/input-not-accepted.md), on
  the consumer's `inputs`.
- **The request takes the root type**: `GenerateRequest<Config>`, or
  `CheckRequest<Config>` for a `check` with `accepts`. Anything else is
  [`tool-request-mismatch`](../errors/tool-request-mismatch.md). `format` takes no
  contract, because a typed value has lost the comments a formatter lays out.
- **The consumer gets no types.** It never reads its config at run time.

The generated module holds the types and
`decode<C: Allocator>(ctx: C, text: Str): Result<Config, Str>`, which runs on
each input before the entry point. JSON inputs reach `decode` as strict JSON,
whatever their dialect; text format inputs as their text. A language of your own
supplies types by answering a `generate` whose `typesOf` is set, with one module
holding the root type and its `decode`.

A `textproto` contract names a schema and a message, and the tool imports the
message from `<tool label>/textproto`:

```textproto schema=build
# tool/routes/BUILD.buri
tool {
    generate {
        accepts: [
            { language: "textproto", type_schema: "routes.proto:Routes" },
        ]
    }
}
```

## The cache

Every answer is an action keyed on:

- the tool's sources and everything they import, as a binary's `link` is keyed;
- the file, and every file the tool `needs`;
- under a contract, the schema and every file it reaches.

Editing the tool, the file, or a schema it read reruns it, and nothing else
does. The tool compiles to JavaScript once per key, kept under
`.buri/out/tools`.
