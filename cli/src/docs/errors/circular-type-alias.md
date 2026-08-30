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
`struct` or an `enum` may refer to itself — that is what makes a list or a tree
expressible — because its fields are a boundary the compiler can stop at. An
alias has no boundary, so a chain of them has to end somewhere else.

## Why

`type Handle = Str` does not introduce a type; it introduces a spelling. Every
place `Handle` is written the compiler substitutes `Str`, and the program is the
one it would have been if `Str` had been written there. So `type A = A;` asks
for a spelling of itself, and substituting it leaves the same question standing.
There is no answer to reach and no fixed point to settle on.

The diagnostic names the whole chain rather than only the declaration it stopped
at, because a cycle of three aliases has no single culprit — any one of the
three is the one to change — and because an exported alias lets the chain cross
a module, where the other half is in a file the reader is not looking at. When
it does cross, each name is printed with the module that declares it and each
of the other declarations carries a span of its own.

The alias resolves to the error type, so the signatures and fields written in
terms of it report nothing further: one cycle is one diagnostic, however many
places name it.

## A program that provokes it

```buri fail code=circular-type-alias
type Celsius = Fahrenheit;
type Fahrenheit = Celsius;

export fn freezing(t: Celsius): Bool {
  t == t
}
```
