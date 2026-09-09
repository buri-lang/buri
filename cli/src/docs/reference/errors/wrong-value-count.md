---
title: A constructor is given the values it holds
message: '`{name}` holds {expected} values, but {given} were given'
fix: 'pass `{name}` the following values: {shape}'
---
# A constructor is given the values it holds

## What to do

Line the call up against the shape the fix prints, which is what the
declaration writes between its parentheses — a variant's too, so a miscounted
`.Some(x)` gets `Some(T)`. The values are positional, so where their types say
which one was left out the fix says that position, and where the call lines up
two ways, or more than one value is missing, it says none of them.

### The position, where the types say which

```text
error: `Row` holds 3 values, but 2 were given [wrong-value-count]
   = fix: `Row` is missing its second value: Row(Int, Str, Bool)
```

### The shape alone, where they do not

```text
error: `Pair` holds 2 values, but 1 were given [wrong-value-count]
   = fix: pass `Pair` the following values: Pair(Int, Int)
```

## A program that provokes it

```buri fail code=wrong-value-count
struct Pair(Int, Int);

fn go(): Pair {
    Pair(1)
}
```
