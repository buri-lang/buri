---
title: A built-in type's name is not declared again
message: '`{name}` is a built-in type and may not be declared'
fix: 'pick another name; in a type, `{name}` always names the built-in type'
---
# A built-in type's name is not declared again

```text
error: `Byte` is a built-in type and may not be declared [built-in-type-name]
```

```buri fail code=built-in-type-name
struct Byte {
    export bits: U8,
}
```

A type annotation reads `Byte` as `U8` wherever it appears, so a struct named
`Byte` could be built but never named as a type. The same holds for every
primitive (`I64`, `Str`, `Bool`, `F32`, ...) and for the aliases `Int`,
`Float`, `Uint` and `Byte`, whether the declaration is a `struct`, an `enum`
or a `type` alias.
