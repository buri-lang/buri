---
title: A method is declared inside an `impl`
message: `{name}` takes `self`, so it is a method
note: a method is found through its receiver's type, so it is declared with that type
fix: move it into an `impl` block for its type, as in `impl Square {{ fn area(self): Int {{ ... }} }}`
---
# A method is declared inside an `impl`

```text
error: `area` takes `self`, so it is a method [method-declared-free]
```

```buri fail code=method-declared-free use=errors
fn perimeter(self): Int {
    self.side * 4
}
```

The `impl` block goes in the module that declares the type.
