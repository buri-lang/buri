---
title: A platform's entry is filled, not called
message: '`{entry}` is the {platform} platform''s entry, and nothing calls it'
note: a platform declares its entries without a body; the program exports one of the same name, and the platform calls that
fix: import the host type instead, and export your own `{entry}`
---
# A platform's entry is filled, not called

```text
error: `main` is the native platform's entry, and nothing calls it [entry-declaration-imported]
```

## What to do

Import only the host type:

```buri
from "native" import { NativeHost };

export fn main(host: NativeHost): Result<(), Str> {
    .Ok(())
}
```

To share code between an entry and its tests, move it into a function the entry
calls with `ctx`.

## A program that provokes it

```buri fail code=entry-declaration-imported
from "native" import { main };
```
