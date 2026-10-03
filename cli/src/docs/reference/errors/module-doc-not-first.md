---
title: `//!` documents the module, so it comes first
message: `//!` documents the module, so it must come first
fix: move it above the first declaration, or write `///` to document the declaration below it
---
# `//!` documents the module, so it comes first

```text
error: `//!` documents the module, so it must come first [module-doc-not-first]
```

```buri fail code=module-doc-not-first
export fn area(side: Int): Int { side * side }

//! This belongs at the top of the file, above everything.
export fn perimeter(side: Int): Int { side * 4 }
```

`///` documents the declaration below it. `//!` documents what contains it,
which at the top of a file is the module. Anywhere lower, it's almost always a
`///` typo.
