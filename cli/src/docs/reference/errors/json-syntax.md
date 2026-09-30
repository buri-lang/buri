---
title: A JSON file is written in its language
message: '{problem}'
fix: '{remedy}'
reproduction: none
---
# A JSON file is written in its language

```text
error: JSON has no comments [json-syntax]
 --> lib/deploy/regions.json:1:1
```

The extension decides what a file may hold:

|                 | `.json` | `.jsonc` | `.json5` |
|---|---|---|---|
| Comments        | no      | yes      | yes      |
| Trailing commas | no      | yes      | yes      |
| JSON5 syntax    | no      | no       | yes      |

JSON5 syntax is single-quoted strings, unquoted keys, hexadecimal numbers,
`Infinity` and `NaN`. A key written twice in one object is an error in all
three, because nothing says which one wins.

A file that does not parse is neither checked nor formatted, and the generator
that lists it does not run.
