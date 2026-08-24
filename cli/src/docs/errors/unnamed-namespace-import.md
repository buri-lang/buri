# A namespace import must be named

```text
error: a namespace import must be named [unnamed-namespace-import]
```

## What to do

write `import * as list`, so every name it brings in is reached through one prefix

## Why

write `import * as name`; bare `import *` is not derivable from the grammar, so that no identifier enters a module's scope without appearing in that module's own source

## A program that provokes it

```buri fail code=unnamed-namespace-import
from "core/list" import *;
```
