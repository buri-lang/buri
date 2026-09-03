---
title: A re-export is written without a leading `export`
message: write `from "..." export {{ ... }}`, without a leading `export`
fix: 'drop the leading `export`: the `export` after the path is the one that re-exports'
---

```buri fail code=re-export-with-a-leading-export
export from "core/list" export { map }
```
