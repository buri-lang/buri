---
title: An exported enum exports every variant
message: a variant carries no `export` of its own; an exported enum exports every one of its variants
note: the enum is the unit of visibility, so a variant's payload fields carry none either
fix: delete the `export`
---

```buri fail code=variant-export
export enum Shape {
  export Circle(Float),
  Square(Float),
}
```
