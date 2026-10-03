---
title: A name is declared once
message: {declaration} is declared twice
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

Rename one of them. Two things with one name in one scope leave a reference
to that name with no answer.
