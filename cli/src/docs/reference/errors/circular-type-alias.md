---
title: A type alias expands to a type, not back to itself
message: 'circular type alias: {cycle}'
note: an alias is transparent, so expanding it has to end at a type; this chain comes back to where it started
fix: 'break the cycle: give one of these a body that is a struct, an enum or a newtype, or point it at a type that is not on the chain'
---
# A type alias expands to a type, not back to itself

```text
error: circular type alias: `A` -> `A` [circular-type-alias]
```

## What to do

Decide which name on the chain is the real type and declare it as one. A
`struct` or an `enum` may refer to itself, because its fields give the compiler
a boundary to stop at; an alias has none.

## Why

`type Handle = Str` introduces a spelling, not a type: everywhere you write
`Handle` the compiler substitutes `Str`. So `type A = A;` substitutes to itself,
and there is nothing to reach.

The diagnostic names the whole chain, because changing any name on it fixes the
error. An exported alias lets the chain cross a module, and then each name is
printed with the module that declares it.

## A program that provokes it

```buri fail code=circular-type-alias
type Celsius = Fahrenheit;

type Fahrenheit = Celsius;

export fn freezing(t: Celsius): Bool {
    t == t
}
```
