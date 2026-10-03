---
title: A type alias never expands to itself
message: 'circular type alias: {cycle}'
note: an alias is transparent, so expanding it has to end at a type; this chain comes back to where it started
fix: 'break the cycle: give one of these a body that is a struct, an enum or a newtype, or point it at a type that is not on the chain'
---
# A type alias never expands to itself

```text
error: circular type alias: `A` -> `A` [circular-type-alias]
```

```buri fail code=circular-type-alias
type Celsius = Fahrenheit;

type Fahrenheit = Celsius;

export fn freezing(t: Celsius): Bool {
    t == t
}
```

`type Handle = Str` is a new spelling, not a new type: the compiler substitutes
`Str` wherever you write `Handle`. A chain that loops never reaches a type.

Changing any name on the chain fixes it, so the diagnostic prints the whole
chain. When it crosses modules, each name carries its declaring module.
