---
title: A schema is one this toolchain can read
message: '{problem}'
fix: '{remedy}'
reproduction: none
---
# A schema is one this toolchain can read

```text
error: `minimum` is a number [json-schema-invalid]
 --> lib/deploy/regions.schema.json:5:20
```

The schema is checked before any data, so its mistakes are reported in the
schema. Two refusals are valid 2020-12:

- A draft-07 keyword with a 2020-12 replacement, such as `dependencies` or
  `additionalItems`. 2020-12 silently ignores it, so data the author meant to
  refuse would pass.
- A `pattern` this toolchain can't read. Patterns are ECMA-262 regular
  expressions over code points, without lookbehind, back-references or `\p{…}`.
  `buri docs guides/json` lists the supported syntax.
