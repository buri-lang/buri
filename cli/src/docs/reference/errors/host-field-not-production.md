---
title: A host's fields are production structs
message: '`{field}` is `{type}`, which is not a production struct'
note: 'the CLI builds the host, so each field is a struct with no fields: one from `platform/host`, or the platform''s own'
fix: '{fix}'
reproduction: none
---
# A host's fields are production structs

```text
error: `count` is `Int`, which is not a production struct [host-field-not-production]
```

```buri ignore why="a platform's surface, compiled only with its rule"
export struct CloudflareHost {
    export alloc: HostAllocator,
    export kv: HostKv,
}

struct HostKv {}
```

A value the entry needs from its host, such as a request, is a parameter of
the entry instead.
