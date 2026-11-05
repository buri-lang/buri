# Two pages

A home page.

```textproto file=cmd/home/BUILD.buri
binary {
    outputs: [
        { platform: "web" },
    ]
}
```

```buri file=cmd/home/main.buri
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    .Ok(())
}
```

An about page, on the same platform.

```textproto file=cmd/about/BUILD.buri
binary {
    outputs: [
        { platform: "web" },
    ]
}
```

```buri file=cmd/about/main.buri
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    .Ok(())
}
```
