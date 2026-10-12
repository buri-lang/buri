# Pages `buri format` lays out

A hidden import sorts in with a visible one and stays hidden.

```buri
# from "core/io" import * as io;
from "core/str" import * as str;

export fn shout(s: Str): Str {
    s
}
```

Hidden lines around the visible ones stay around them.

```buri
# export fn demo(): I64 {
    let x = 1;
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

So is one in the middle of a fence its hidden lines enclose, and a hidden
line that changes how a visible one is laid out.

```buri
# export fn six(): I64 {
let   x = 6;
# let z = 2;
x
# }
```

```buri
export fn  nine(): I64 {
    let n =
#        4 +
        5;
    n
}
```

A hidden blank line hides nothing.

```buri
export fn seven(): I64 {
    7
}
```

A line that starts `##` shows `#`, and stays escaped.

```buri
/*
## not hidden
*/
export fn eight(): I64 {
    8
}
```

```buri
# from "core/io" import * as io;

/*
## shown
*/
export fn ten(): I64 {
    10
}
```

Statements are laid out as the body of a function.

```buri wrap=body
let a = 1;
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