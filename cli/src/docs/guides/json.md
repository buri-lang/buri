# Check JSON against a schema

List a JSON file in a generator's `inputs`, and the build checks it against its
schema before the generator reads it:

```textproto schema=build
library {
    generators: [
        { tool: "//tools/routes", inputs: ["regions.json"] },
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

A mistake is reported where it is, and the generator does not run:

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

`buri build`, `buri test` and `buri lint` all run the check, `buri format` lays
the file out, and the language server checks it as you type. The check and the
formatter are `std/json`, the tool this toolchain ships for JSON; `//tools/routes`
is a [tool](../reference/build/tools.md) of your own with a `generate` entry
point.

## Which files

Only a file some rule's `inputs` lists. A `package.json` or a config your
program reads at run time is left alone.

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

- **Every file names one.** A top-level `"$schema"` is required.
- **It is checked in.** `"$schema"` is a path relative to the file, or a `//`
  path from the repository root. Nothing is fetched, so a URL is refused.
- **It is JSON Schema 2020-12.** A schema's own `"$schema"` is
  `https://json-schema.org/draft/2020-12/schema`, the one URL known by name.
  A file with that `"$schema"` is a schema, and is checked as one.
- **`"$schema"` is a property like any other.** A schema with
  `"additionalProperties": false` lists it in `properties`.
- **`$ref` stays in the repository.** It reaches a pointer or an anchor in its
  own file, another checked-in file by a relative or `//` path, or a schema by
  its absolute `$id`.

Every keyword of the core, applicator, unevaluated and validation vocabularies
is enforced. `format`, `contentMediaType` and the annotations such as `title`
and `default` assert nothing, as 2020-12 says.

`pattern` and `patternProperties` hold ECMA-262 regular expressions, matched
anywhere in the string. This much of the syntax is read:

```text
a|b  (x)  (?:x)  (?<name>x)  (?=x)  (?!x)
*  +  ?  {n}  {n,}  {n,m}, each greedy or lazy with a trailing `?`
.  ^  $  \b  \B  [a-z]  [^0-9]  \d \D \w \W \s \S
\t \n \r \v \f \0  \xHH  \uHHHH  \u{H…}  \cX
```

Lookbehind, back-references and `\p{…}` are refused, rather than read as
something else.

JSON5's `Infinity`, `-Infinity` and `NaN` are numbers: they pass
`"type": "number"`, and fail `"type": "integer"` and every `minimum`,
`maximum` and `multipleOf`.

## Formatting

`buri format` lays a file out at the width and indent every `.buri` file gets,
and keeps every comment. It never adds `"$schema"`. A file that does not parse
is left as it is, and `buri format --check` names it.

```json
{ "$schema": "regions.schema.json", "regions": ["eu-west", "us-east"] }
```

## Generating types

`std/json` in `generators` gives a module named after each file:

```textproto schema=build
library {
    generators: [
        { tool: "std/json", inputs: ["regions.schema.json", "regions.json"] },
    ]
}
```

```buri ignore why="it imports modules the build generates from the JSON files"
from "//lib/deploy/regions.json" import { Regions, regions };
from "//lib/deploy/regions.schema.json" import { Region };
```

- **A schema file** gives the types it describes.
- **A data file** gives the types its `"$schema"` describes, and its contents
  as `export let regions: Regions`, named after the file.
- **The root type** is named after the schema's `title`, or its file name up
  to the first `.` when there is none.

A tool with a [contract](../reference/build/tools.md#input-contracts) gets the
same types, generated into the tool, with a `decode` for the values it is
handed.

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
- **Not every property is a field.** `"$schema"` says where the schema is, and
  a property with a `const` has one value, so neither gets a field. That is
  also what drops the tag from a `oneOf`'s variants.
- **Validation stays the check's.** `pattern`, `minimum`, `format`,
  `minLength` and the rest describe values, not types, and the check has
  already enforced them.
- JSON5's `Infinity`, `-Infinity` and `NaN` read as `F64`.

A construct no one Buri type follows is refused where the schema writes it,
as [`json-schema-no-type`](../reference/errors/json-schema-no-type.md): `if`,
`then` and `else`; `allOf`; an `anyOf`, or a `oneOf` with no `const` tag;
`patternProperties`; `dependentSchemas`; `additionalProperties` holding a
schema beside `properties`; `unevaluatedProperties` or `unevaluatedItems`
holding a schema; `prefixItems`; `$dynamicRef`; an `enum` of anything but
strings; and a `type` of several types.
