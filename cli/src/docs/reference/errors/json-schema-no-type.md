---
title: A schema that generates types maps to Buri types
message: '`{keyword}` has no Buri type'
note: '{why}'
fix: 'describe the value with a construct that has a type'
reproduction: none
---
# A schema that generates types maps to Buri types

```json
{ "if": { "required": ["port"] }, "then": { "required": ["host"] } }
```

`json` turns a schema into types for a contract or a `generators` entry.
Keywords about values, such as `pattern`, `minimum` and `format`, stay the
check's. These change a value's shape in a way no one type follows, so they
are refused where they are written:

- `if`, `then` and `else`;
- `allOf`, and an `anyOf` that is not one type or `null`;
- a `oneOf` whose branches no `const` string tells apart;
- `patternProperties` and `dependentSchemas`;
- `additionalProperties` holding a schema beside `properties`;
- `unevaluatedProperties` and `unevaluatedItems` holding a schema;
- `prefixItems` and `$dynamicRef`;
- an `enum` of anything but strings, and a `type` of several types.

`buri docs guides/json` has the whole mapping.
