---
title: An output enters through a function its binary exports
message: '{package} exports no `{entry}`'
fix: export `{entry}` from its `main.buri`, or point this output at a function it does export
reproduction: none
---
# An output enters through a function its binary exports

```text
error: //cmd/site exports no `mainForNode` [entry-not-found]
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

An `outputs` entry names the function that artifact starts at. The name is
written in the build file and the function is written in `main.buri`, so the two
can disagree. This is where they are compared.

The page lists what `main.buri` does export, because the mistake is almost
always a spelling — and where one of them is a near miss the fix names it:
"if you meant `fetch`, use that; if not, export `fetsh` from its `main.buri`".

An output whose `entries` names nothing enters through `main`, and a binary
with no `main` gets `no-main` instead. Those are different mistakes: one binary has not
written its entry point, the other wrote the name twice and spelled it
differently once.
