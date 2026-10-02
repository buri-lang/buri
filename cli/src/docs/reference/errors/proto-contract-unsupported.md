---
title: A `.proto` file is taken as text
message: '`{tool_entry}` cannot take `proto` inputs typed: a `.proto` file is a schema, not a value'
fix: 'accept `textproto` instead, whose files are values of the message: `{{ language: "textproto", type_schema: "config.proto:Config" }}`'
reproduction: none
---
# A `.proto` file is taken as text

```textproto schema=build
tool {
    generate {
        accepts: [
            { language: "proto", type_schema: "config.proto:Config" },
        ]
    }
}
```

A contract hands a tool each input as a value of the contract's root type. A
`.proto` file declares messages and holds none, so there is no `Config` to read
out of it.

The values live in text format files. Accept those:

```textproto schema=build
tool {
    generate {
        accepts: [
            { language: "textproto", type_schema: "config.proto:Config" },
        ]
    }
}
```

Or leave `proto` out of `accepts`, and `generate` gets the schema's text.
