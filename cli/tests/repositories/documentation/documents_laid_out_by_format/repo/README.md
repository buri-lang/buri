# Pages `buri format` lays out

A hidden import sorts in with a visible one and stays hidden.

```buri
from "core/str" import * as str;
# from "core/io" import * as io;

export fn   shout(s: Str): Str { s }
```

Hidden lines around the visible ones stay around them.

```buri
# export fn demo(): I64 {
let   x = 1;
x
# }
```

A hidden line in the middle of lines that aren't a module is left for a
person.

```buri
let   y = 1;
# let z = 2;
y
```

Statements are laid out as the body of a function.

```buri wrap=body
let a =   1;
```

```buri wrap=body
let b = ;
```

- A fence in a list keeps the list's indent.

  ```buri
  export fn two(): I64 {
        2
  }
  ```

Nothing to lay out: an empty fence, one that doesn't parse, an expression,
and one already laid out.

```buri
```

```buri
export fn (
```

```buri wrap=expr
1 +    1
```

```buri
export fn one(): I64 {
    1
}
```

Build files in a page are printed the way a `BUILD.buri` is.

```textproto schema=build
library {
  sources: ["a.buri"]
}
```

- In a list too.

  ```textproto schema=build
  library {
          sources: ["b.buri"]
  }
  ```

```textproto schema=build
library {
    sources: ["c.buri"]
}
```

```textproto fail code=build-unknown-field file=BUILD.buri
library {
  bogus: 1
}
```

```textproto schema=build
library {
```
