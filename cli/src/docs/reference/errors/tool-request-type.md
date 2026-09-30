---
title: A tool with a contract takes its root type
message: '`{entry}` takes `{expected}` under its `{language}` contract'
note: 'the types of the contract are the module `{module}`, and each input arrives as its root type'
fix: 'import the root type from `{module}` and write `{expected}`'
reproduction: none
---
# A tool with a contract takes its root type

```buri ignore why="it imports the module the build generates into the tool from its contract"
from "core/effect" import { Allocator };
from "core/tool" import { Generated, GenerateRequest };
from "//tools/database_schema_codegen/json" import { Config };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Config>): Generated {
    Generated { modules: [], diagnostics: [], needs: [] }
}
```

With `accepts`, the build generates the contract's types into the tool and
hands the entry point typed values. Without it, the entry point takes `Str`.
