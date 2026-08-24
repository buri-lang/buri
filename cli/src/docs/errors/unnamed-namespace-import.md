# A namespace import must be named

```text
error: a namespace import must be named [unnamed-namespace-import]
```

## What to do

write `import * as list`, so every name it brings in is reached through one prefix

## Why

Bare `import *` is not derivable from the grammar at all, so no identifier can
enter a module's scope without appearing in that module's own source. The path
leads for the same kind of reason: an editor knows which module you mean before
you open the brace, and can complete the specifier list.

## A program that provokes it

```buri fail code=unnamed-namespace-import
from "core/list" import *;
```
