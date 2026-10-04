---
title: A host's fields are production structs
message: '`{field}` is `{type}`, which is not a production struct'
note: 'the CLI builds the host, so each field is a struct with no fields: one from `platform/host`, or one the platform declares'
fix: '{fix}'
reproduction: none
---
# A host's fields are production structs

```text
error: `count` is `I64`, which is not a production struct [host-field-not-production]
```

```buri repo=cli/tests/docs/repositories/worker file=platform/cloudflare_worker/platform.buri
# from "platform/effect" import { Request, Response };
# from "platform/host" import { HostAllocator };
#
export struct CloudflareHost {
    export alloc: HostAllocator,
    export kv: HostKv,
}

struct HostKv {}
#
# export fn fetch(host: CloudflareHost, request: Request): Response;
```

A value the entry needs from its host, such as a request, is a parameter of
the entry instead.
