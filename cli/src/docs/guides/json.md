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
