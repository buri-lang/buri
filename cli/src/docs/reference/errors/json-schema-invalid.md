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

Every keyword a schema writes is checked before any file is checked against
it, so a mistake in the schema is reported in the schema rather than as a
puzzling verdict on the data.

Two kinds of refusal are not mistakes in 2020-12 terms:

- **A draft-07 keyword with a 2020-12 replacement**, such as `dependencies` or
  `additionalItems`. 2020-12 would silently ignore it, so the check would pass
  data the author meant to refuse.
- **A `pattern` this toolchain cannot read.** Patterns are ECMA-262 regular
  expressions over code points. Lookbehind, back-references and `\p{…}` are
  not supported. `buri docs guides/json` lists the syntax that is.
