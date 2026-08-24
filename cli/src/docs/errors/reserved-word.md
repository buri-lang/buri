# Reserved words are not identifiers

```text
error: `return` is a reserved word and may not be used as an identifier [reserved-word]
```

## What to do

pick another name; `return` is not available

## Why

reserved for a future version of Buri; see grammar.ebnf, ReservedWord

## A program that provokes it

```buri fail code=reserved-word
fn return(n: Int): Int { n }
```
