---
title: A value is one its field holds
message: '{problem}'
fix: '{remedy}'
reproduction: none
---
# A value is one its field holds

```textproto ignore why="a data file, not a build file"
port: "80"

# an `int32` takes a whole number
port: 99999999999

# that fits 32 bits
tier: SILVER

# an enum takes one of its values' names, or a number
limits: 4

# a message is written `limits { ... }`
name: ["a", "b"]
# a field that is not `repeated` takes one value
```

| Field | Value |
|---|---|
| `string` | a quoted string whose bytes are UTF-8 |
| `bytes` | a quoted string |
| `bool` | `true`, `True`, `t`, `false`, `False`, `f`, `1` or `0` |
| the integers | a decimal, `0x` hex or `0` octal number in the type's range |
| `float`, `double` | a number, or `inf`, `infinity` or `nan` |
| an enum | a value's name, or a 32-bit number |
| a message | `{ ... }` or `< ... >` |

A `uint64` or `fixed64` past what a Buri `Int` holds is refused, because the
generated field is an `Int`.
