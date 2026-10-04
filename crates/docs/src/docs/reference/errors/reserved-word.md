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

```buri fail code=reserved-word
fn return(n: Int): Int { n }
```

`buri docs grammar` lists every reserved word under `ReservedWord`.
