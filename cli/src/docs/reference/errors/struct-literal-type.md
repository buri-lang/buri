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
answers. Which reads better is a question about the line, not about the rule.

## Why

The compiler reads an anonymous literal's type from above and never solves for
it. That is the whole claim the shorter syntax makes: the type is already
written down somewhere you can see, so leaving it out of the literal costs you
nothing. A literal that worked its type out from its own fields would be a
search instead, and the answer would depend on the order the checker happened to
visit the expression in.

So the rule is deliberately narrow. The expected type reaches a literal in a
`let` with an annotation, an argument of a call, the value of a field, a match
arm and a function's result — the places a type is already spelled out. Nowhere
else, and never from the fields.

The compiler accepts a generic struct only where every one of its type arguments
is settled. `Holder<Int>` is a type you can see. `Holder<?>`, where the argument
is still an inference variable, is one the fields would have to decide, and
deciding it here is the inference this is not.

An enum is not a struct. A type alone does not say which variant a literal
builds, so `.Variant { ... }` names it and stays the way to write one.

## Which braces are a literal at all

Separately from this rule, the grammar decides what the braces *are*, and it
decides on two tokens. A `{` followed by a `..` or by a `name :` opens a
literal; every other `{` opens a block. So `{ }`, `{ hi }` and `{ hi, hello }`
are blocks, and a literal whose first field is shorthand keeps its type name —
`World { hi, hello }`. Shorthand after a first keyed field is fine:
`{ hi: hi, hello }`.

## A program that provokes it

```buri fail code=struct-literal-type
struct World {
    export hi: Str,
}

fn build(): Int {
    let w = { hi: "hi" };
    w.hi.length()
}
```
