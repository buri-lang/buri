---
title: An untyped literal needs an expected type
message: nothing here says what this literal builds
note: a `{{ ... }}` with no type in front of it is given one by what it is checked against, and that is the only place it looks
---
# An untyped literal needs an expected type

```text
error: nothing here says what this literal builds [untyped-struct-literal]
```

```buri fail code=untyped-struct-literal
struct World {
    export hi: Str,
}

fn build(): Int {
    let w = { hi: "hi" };
    w.hi.length()
}
```

Write the type in front of the `{`, or annotate the binding:
`World { hi: "hi" }` and `let w: World = { hi: "hi" };` both work.

The expected type reaches a literal only from an annotated `let`, a call
argument, a field value, a match arm or a function's result. The compiler never
infers it from the fields.

A generic struct needs every type argument settled: `Holder<Int>` works, but a
`Holder` whose argument is still being inferred doesn't.

An enum literal still needs `.Variant { ... }`, because a type alone doesn't say
which variant it builds.

The grammar decides what braces are from two tokens: a `{` followed by `..` or
`name :` opens a literal, and any other `{` opens a block. So `{ }`, `{ hi }` and
`{ hi, hello }` are blocks, and a literal whose first field is shorthand needs
its type name: `World { hi, hello }`. Shorthand after a keyed first field is
fine: `{ hi: hi, hello }`.
