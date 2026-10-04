---
title: An output enters through a function its binary exports
message: '{package} exports no `{entry}`'
fix: export `{entry}` from its `main.buri`, or point this output at a function it does export
reproduction: none
---
# An output enters through a function its binary exports

```text
error: //cmd/site exports no `mainForNode` [unknown-entry-function]
```

## What to do

Export the function the output names, or name one the binary already exports.

```buri role=entry
from "node" import { NodeHost };
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    .Ok(())
}

export fn mainForNode(host: NodeHost): Result<(), Str> {
    .Ok(())
}
```

```textproto schema=build
binary {
    outputs: [
        { platform: "web" },
        { platform: "node", entries: [
            { name: "main", function: "mainForNode" },
        ] },
    ]
}
```

## Why

The build file names the function and `main.buri` declares it, so the two can
disagree. The error lists what `main.buri` does export, and names a near miss:
"if you meant `fetch`, use that". An output with no `entries` enters through
`main`, and a binary without one is `missing-main`.
