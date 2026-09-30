# Tools

A tool is a program the build runs on a language's files. It is a rule of its
own, a peer of `library` and `binary`:

```textproto schema=build
# tools/lines/BUILD.buri
tool {
    sources: ["words.buri"]

    check {}
    format {}
}
```

Its root is `tool.buri`, which exports one function per block:

```buri
// tools/lines/tool.buri
from "core/effect" import { Allocator };
from "core/format" import { Doc };
from "core/tool" import { Checked, CheckRequest, Diagnostic, FormatRequest };

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
from "core/effect" import { Allocator };
from "core/tool" import { Generated, GenerateRequest };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    Generated { modules: [], diagnostics: [], needs: [] }
}
```

- **Blocks and exports match.** A block without its function is
  [`tool-entry-point-not-exported`](../errors/tool-entry-point-not-exported.md),
  and an exported `check`, `format` or `generate` without its block is
  [`tool-entry-point-undeclared`](../errors/tool-entry-point-undeclared.md).
- **`ctx` has `Allocator` and nothing else.** A bound naming any other effect
  is [`tool-context-beyond-allocator`](../errors/tool-context-beyond-allocator.md),
  so what a tool answers is a function of what it was handed.
- **There is no `main`.** The toolchain writes one that reads the request,
  calls the entry point and writes the answer, so a tool never sees `Stdin` or
  `Stdout`.
- `sources`, `dependencies` and `test` mean what they mean on a `binary`. A
  tool's own tests import `tool.buri` as `//tools/lines/tool.buri`; nothing
  else imports a tool's modules
  ([`tool-source-import`](../errors/tool-source-import.md)).
- `buri gen` leaves a package with a `tool` rule as written.

## Who calls which entry point

| Entry point | Called for |
| --- | --- |
| `check` | Each file a rule's `inputs` lists, in a language whose `check` names the tool. Before any generator reads it. |
| `format` | The same files, in a language whose `format` names the tool, by `buri format` and your editor. |
| `generate` | Each `generators` entry naming the tool. See [`generators.md`](./generators.md). |

A language names its tools in [`REPO.buri`](./repo-config.md). A reference to
a tool without that entry point is
[`tool-without-entry-point`](../errors/tool-without-entry-point.md), and a name
that is no tool is [`no-such-tool`](../errors/no-such-tool.md).

The toolchain ships two: `std/json` (`check`, `format` and `generate`, for
`json`, `jsonc` and `json5`) and `std/proto` (`generate`, for `.proto`
schemas). The old
`std/codegen/proto` is [`retired-tool-name`](../errors/retired-tool-name.md).

## What an entry point is handed

`core/tool` has the types; `buri docs core/tool` prints them. Each input is an
`Input<Str>`: its repository `path`, its `language`, and its text as `value`.
Under a [contract](#input-contracts) it is an `Input<Root>` instead.

- **`check`** answers `Checked`: its diagnostics, and the repository paths it
  `needs`. A file it needs, such as a schema, comes back in `files` on the next
  call. A path outside the repository is
  [`schema-not-local`](../errors/schema-not-local.md).
- **`format`** answers a `core/format` `Doc`, and the toolchain lays it out at
  the margin and indent every `.buri` file gets. A file the tool cannot read
  gets diagnostics and no doc, and stays as it is.
- **`generate`** answers modules, diagnostics and `needs`, as
  [`generators.md`](./generators.md) describes.

A diagnostic's `code` prints under its catalogue page when there is one, with
the tool's own sentence, and as
[`tool-diagnostic`](../errors/tool-diagnostic.md) otherwise. A tool that does
not build, or stops without answering, is
[`tool-failed`](../errors/tool-failed.md).

## Input contracts

A tool whose inputs always follow one schema says so in its own `BUILD.buri`:

```textproto schema=build
# tools/database_schema_codegen/BUILD.buri
tool {
    sources: ["emit.buri"]

    generate {
        accepts: [{ language: "json", type_schema: "config.schema.json" }]
    }
}
```

The consumer lists its file as usual, and the file may leave out `"$schema"`:

```textproto schema=build
# lib/orders/BUILD.buri
library {
    generators: [
        { tool: "//tools/database_schema_codegen", inputs: ["schema.json"] },
    ]
}
```

The build generates the schema's types into the tool as the module
`<tool label>/<language>`, and `generate` takes them:

```buri ignore why="it imports the module the build generates into the tool from its contract"
// tools/database_schema_codegen/tool.buri
from "core/effect" import { Allocator };
from "core/tool" import { Generated, GenerateRequest };
from "//tools/database_schema_codegen/json" import { Config };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Config>): Generated {
    // request.inputs[i].value is a Config
    Generated { modules: [], diagnostics: [], needs: [] }
}
```

- **The check uses the contract's schema.** `std/json` checks `schema.json`
  against `config.schema.json`, wherever the tool is used. The file's own
  `"$schema"` may be left out or name the same schema; anything else is
  [`schema-mismatch`](../errors/schema-mismatch.md), and so is one file read
  by two tools with different contracts.
- **`type_schema` belongs to the language.** For `json`, `jsonc` and `json5`
  it is a JSON Schema path relative to the tool's package, or a `//` path.
  [`guides/json`](../../guides/json.md#generating-types) has the type mapping.
- **One entry per language.** An input in a language no entry lists is
  [`input-language-not-accepted`](../errors/input-language-not-accepted.md), on
  the consumer's `inputs`.
- **The request takes the root type.** `GenerateRequest<Config>`, or
  `CheckRequest<Config>` for a `check` with `accepts`; anything else is
  [`tool-request-type`](../errors/tool-request-type.md). `format` takes no
  contract, because a typed value has lost the comments a formatter lays out.
- **The consumer gets no types.** It never reads its config at run time.

The module holds the types and a `decode`, which the `main` the build writes
calls on each input before the entry point sees it:

```buri sig
export fn decode<C: Allocator>(ctx: C, text: Str): Result<Config, Str>
```

A JSON input arrives as strict JSON, whatever its dialect, so comments and
JSON5 syntax never reach `decode`. A language of your own supplies types by
answering a `generate` whose `typesOf` is set: one module, with the root type
and this `decode`, which reads the input's text.

## The cache

Every answer is an action keyed on the tool's program and the whole request:

- the tool's sources and everything they import, the way a binary's `link` is
  keyed;
- the file, and every file the tool `needs`;
- under a contract, the schema and every file it reaches, which also decide
  the types the tool is built with.

So editing the tool, the file, or a schema it read asks again, and nothing else
does. The tool is compiled to JavaScript once per key and kept under
`.buri/out/tools`.
