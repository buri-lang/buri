---
title: An anonymous literal takes its type from its surroundings
message: nothing here says what this literal builds
note: a `{{ ... }}` with no type in front of it is given one by what it is checked against, and that is the only place it looks
---
# An anonymous literal takes its type from its surroundings

```text
error: nothing here says what this literal builds [struct-literal-type]
```

## What to do

Write the type in front of the `{`. An annotation on the binding does the same
job, so `let w: World = { hi: "hi" };` and `World { hi: "hi" }` are both
answers; which reads better is a question about the line, not about the rule.

## Why

The type of an anonymous literal is read from above and never solved for. That
is the whole claim the shorter syntax makes: the type is already written down
somewhere the reader can see, and leaving it out of the literal costs them
nothing. A literal that had to work its type out from its own fields would not
be that — it would be a search, and the answer would depend on the order the
checker happened to visit the expression in.

So the rule is deliberately narrow. The expected type reaches a literal in a
`let` with an annotation, an argument of a call, the value of a field, a match
arm and a function's result — the places a type is already spelled out. Nowhere
else, and never from the fields.

A generic struct is accepted only where every one of its type arguments is
settled. `Holder<Int>` is a type a reader can see. `Holder<?>`, where the
argument is still an inference variable, is one the fields would have to
decide, and deciding it here is the inference this is not.

An enum is not a struct: a type alone does not say which variant a literal
builds, so `.Variant { ... }` names it and stays the way to write one.

## Which braces are a literal at all

Separately from this rule, the grammar decides what the braces *are*, and it
decides on two tokens: a `{` followed by a `..` or by a `name :` opens a
literal, and every other `{` opens a block. So `{ }`, `{ hi }` and
`{ hi, hello }` are blocks, and a literal whose first field is shorthand keeps
its type name — `World { hi, hello }`. Shorthand after a first keyed field is
fine: `{ hi: hi, hello }`.

## A program that provokes it

```buri fail code=struct-literal-type
struct World { export hi: Str }

fn build(): Int {
  let w = { hi: "hi" };
  w.hi.length()
}
```
