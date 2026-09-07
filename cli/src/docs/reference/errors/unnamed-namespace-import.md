---
title: A namespace import must be named
message: a namespace import must be named
note: write `import * as name`; bare `import *` is not derivable from the grammar, so that no identifier enters a module's scope without appearing in that module's own source
fix: write `import * as list`, so every name it brings in is reached through one prefix
---
# A namespace import must be named

```text
error: a namespace import must be named [unnamed-namespace-import]
```

## What to do

Name the import. Bare `import *` is not derivable from the grammar at all, so no
identifier can enter a module's scope without appearing in that module's own
source.

## A program that provokes it

```buri fail code=unnamed-namespace-import
from "core/list" import *;
```
