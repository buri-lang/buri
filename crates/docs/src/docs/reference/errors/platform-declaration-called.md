---
title: A program calls functions with bodies, not its platform's declarations
message: '`{name}` is declared by `{platform}` with no body, so there is nothing to call'
note: a platform's `platform.buri` declares its entries for the platform to call, and nothing implements a declaration with no body
fix: call a function with a body. The program fills an entry with a function of its own, and the platform calls it
reproduction: none
---
# A program calls functions with bodies, not its platform's declarations

```text
error: `load` is declared by `//platform/kit` with no body, so there is nothing to call [platform-declaration-called]
```

An entry's declaration is the signature a platform calls. The program fills it
with a function of its own, named in its `BUILD.buri`, and calls that function
directly when it needs to:

```text
binary {
    outputs: [
        { platform: "//platform/kit", entries: [{ name: "load", function: "fill" }] },
    ]
}
```
