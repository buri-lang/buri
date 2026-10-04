# Check JSON against a schema

List a JSON file in a generator's `inputs`, and the build checks it against its
schema:

```textproto schema=build
library {
    generators: [
        { tool: "//tool/routes", inputs: ["regions.json"] },
    ]
}
```

```json
{
    "$schema": "regions.schema.json",
    "regions": ["eu-west", "us-east"]
}
```

```json
{
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
        "$schema": { "type": "string" },
        "regions": { "type": "array", "items": { "type": "string" } }
    },
    "required": ["regions"],
    "additionalProperties": false
}
```

A mistake stops the generator:

```text
error: expected a string, found an integer [json-schema-violation]
 --> lib/deploy/regions.json:3:28
  |
3 |     "regions": ["eu-west", 7]
  |                            ^
  |
  = note: the schema says so at `lib/deploy/regions.schema.json#/properties/regions/items/type`
  = fix: write a string here
```

`buri build`, `buri test`, `buri lint` and the language server run the check.
The check and formatter come from the built-in `json` tool; `//tool/routes` is
a [tool](../reference/build/tools.md) of your own with a `generate` entry
point. Built-in tools have bare names, and yours are `//label`s.

## Which files

Only files a rule's `inputs` lists. A `package.json` or a runtime config is
left alone.

The extension decides the language:

|                 | `.json`      | `.jsonc` | `.json5`                                |
|---|---|---|---|
| Comments        | syntax error | kept     | kept                                    |
| Trailing commas | syntax error | removed  | written when a list breaks across lines |

`REPO.buri` can give a language more extensions:

```textproto schema=repo
language {
    name: "jsonc"
    extensions: [".code-workspace"]
}
```

## The schema

- **Every file names one** with a top-level `"$schema"`.
- **It's checked in.** `"$schema"` is relative to the file, or a `//` path.
  Nothing is fetched, so a URL is refused.
- **It's JSON Schema 2020-12.** A schema's own `"$schema"` is
  `https://json-schema.org/draft/2020-12/schema`, the one URL allowed. A file
  with that `"$schema"` is checked as a schema.
- **`"$schema"` is an ordinary property.** A schema with
  `"additionalProperties": false` must list it in `properties`.
- **`$ref` stays in the repository.** It reaches a pointer or anchor in its own
  file, another checked-in file by relative or `//` path, or a schema by its
  absolute `$id`.

Every keyword of the core, applicator, unevaluated and validation vocabularies
is enforced. `format`, `contentMediaType` and annotations like `title` assert
nothing, per 2020-12.

`pattern` and `patternProperties` are ECMA-262 regular expressions, matched
anywhere in the string. Supported syntax:

```text
a|b  (x)  (?:x)  (?<name>x)  (?=x)  (?!x)
*  +  ?  {n}  {n,}  {n,m}, each greedy or lazy with a trailing `?`
.  ^  $  \b  \B  [a-z]  [^0-9]  \d \D \w \W \s \S
\t \n \r \v \f \0  \xHH  \uHHHH  \u{H…}  \cX
```

Lookbehind, back-references and `\p{…}` are refused.

JSON5's `Infinity`, `-Infinity` and `NaN` are numbers: they pass
`"type": "number"`, and fail `"type": "integer"` and every `minimum`,
`maximum` and `multipleOf`.

## Formatting

`buri format` uses `.buri` width and indent, and keeps every comment. It never
adds `"$schema"`. A file that doesn't parse is left alone, and
`buri format --check` names it.

```json
{ "$schema": "regions.schema.json", "regions": ["eu-west", "us-east"] }
```

## Generating types

`json` in `generators` gives a module named after each file:

```textproto schema=build repo=cli/tests/docs/repositories/json-guide file=lib/deploy/BUILD.buri
library {
    generators: [
        { tool: "json", inputs: ["regions.schema.json", "regions.json"] },
    ]
}
```

```buri repo=cli/tests/docs/repositories/json-guide file=lib/deploy/lib.buri
from "//lib/deploy/regions.json" import { regions };
from "//lib/deploy/regions.schema.json" import { Regions };
```

- **A schema file** gives the types it describes.
- **A data file** gives the types its `"$schema"` describes, and its contents
  as `export let regions: Regions`, named after the file.
- **The root type** is named after the schema's `title`, or else its file
  name up to the first `.`.

A tool with a [contract](../reference/build/tools.md#input-contracts) gets the
same types, plus a `decode` for the values it's handed.

### The mapping

| JSON Schema | Buri |
|---|---|
| `"type": "object"` with `properties` | a struct, one exported field per property |
| a property in `required` | the field's type |
| any other property | `Option` of it |
| `"type": "array"` with `items` | `[T]` |
| `"enum"` of strings | an enum, one variant per string |
| `$ref` to `$defs` | the named type, declared once |
| `oneOf` of objects told apart by one required `const` string | an enum, a variant per branch carrying its struct |
| `"string"`, `"integer"`, `"number"`, `"boolean"` | `Str`, `Int`, `F64`, `Bool` |
| `"null"` beside one other type, in `type` or `anyOf`/`oneOf` | `Option` of that type |
| `"type": "null"` alone | `()` |
| `"object"` with only `additionalProperties` | `[(Str, T)]`, in document order |
| `{}`, `true`, or an object with neither | `core/json`'s `Json` |
| `const` | the constant's type |

```json
{
    "title": "Config",
    "type": "object",
    "properties": {
        "name": { "type": "string" },
        "port": { "type": "integer", "minimum": 1 },
        "tier": { "enum": ["free", "pro-plus"] }
    },
    "required": ["name", "tier"]
}
```

```buri
export struct Config {
    export name: Str,
    export port: Option<Int>,
    export tier: ConfigTier,
}

export enum ConfigTier {
    Free,
    ProPlus,
}
```

- **Names.** A type is named after its `title`, then its `$defs` name, then
  its place: `tier` inside `Config` is `ConfigTier`. Fields and variants are
  camel case, and a keyword gets a trailing `_`, so `"type"` is `type_`.
- **Every type derives `Equal` and `Show`.**
- **`"$schema"` and `const` properties get no field.** That's also what drops
  the tag from a `oneOf`'s variants.
- **Validation keywords** like `pattern` and `minimum` don't affect types; the
  check already enforced them.
- JSON5's `Infinity`, `-Infinity` and `NaN` read as `F64`.

A construct with no single Buri type is refused as [`json-untyped-keyword`](../reference/errors/json-untyped-keyword.md): `if`,
`then` and `else`; `allOf`; an `anyOf`, or a `oneOf` with no `const` tag;
`patternProperties`; `dependentSchemas`; `additionalProperties` holding a
schema beside `properties`; `unevaluatedProperties` or `unevaluatedItems`
holding a schema; `prefixItems`; `$dynamicRef`; an `enum` of anything but
strings; and a `type` of several types.
