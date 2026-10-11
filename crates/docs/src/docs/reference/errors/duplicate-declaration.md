---
title: A name is declared once
message: '{declaration} is declared twice'
---
# A name is declared once

```text
error: variant `Yes` is declared twice [duplicate-declaration]
```

```buri fail code=duplicate-declaration
enum Choice {
    Yes,
    No,
    Yes,
}
```

```buri fail code=duplicate-declaration
struct Point {
    export x: Int,
}

struct Point {
    export y: Int,
}
```

```buri fail code=duplicate-declaration
fn size(a: Int): Int {
    a
}

fn size(a: Str): Int {
    0
}
```

Rename one of them. Two things with one name in one scope leave a reference
to that name with no answer. There is no overloading: two functions of one
name are refused like two structs. Methods in different `impl`s are fine,
because a receiver picks one.
