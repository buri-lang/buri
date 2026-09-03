---
title: Reserved words are not identifiers
message: `{word}` is a reserved word and may not be used as an identifier
note: reserved for a future version of Buri; see grammar.ebnf, ReservedWord
fix: pick another name; `{word}` is not available
---
# Reserved words are not identifiers

```text
error: `return` is a reserved word and may not be used as an identifier [reserved-word]
```

## What to do

Pick another name.

## Why

The word is reserved for a future version of the language rather than used by
this one, so it is refused now instead of becoming a source-breaking change
later. `buri docs grammar` lists the whole set under `ReservedWord`.

## A program that provokes it

```buri fail code=reserved-word
fn return(n: Int): Int { n }
```
