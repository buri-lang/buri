---
title: A generator is a tool rule
message: '`{tool}` is a binary, and a generator is a `tool` rule'
note: the build calls a tool's `generate` itself, so a generator has no `main` and reads no standard input
fix: 'make it a `tool` rule under //tools/: move `main.buri` to `tool.buri`, export `generate`, and declare `generate {{}}`'
reproduction: none
---
# A generator is a tool rule

```textproto schema=build
# tools/routes/BUILD.buri
tool {
    generate {}
}
```

```buri
from "core/tool" import { Generated, GenerateRequest };

// tools/routes/tool.buri
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    Generated { modules: [], diagnostics: [], needs: [] }
}
```

What `main` did with `core/codegen`'s `run` becomes the body of `generate`.
Each input arrives with its `path` and its text as `value`. See
[`tools.md`](../build/tools.md).
