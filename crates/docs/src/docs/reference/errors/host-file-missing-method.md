---
title: A `js` file implements every method its structs declare
message: '`{file}` {gap}'
note: an export named after one of the platform's production structs implements it, written `export const S = {{ ... }}` with one function per method
fix: export the struct as an object with one function per method, `self` first
reproduction: none
---
# A `js` file implements every method its structs declare

```text
error: `fetch.mjs` exports `HostKv` without `put`, which `Kv` declares with 4 parameters [host-file-missing-method]
```

Each method a platform's `platform.buri` declares without a body is the entry's
`js` file's to implement, under the struct's name, with the same parameter count:

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_kv/repo file=platform/cloudflare_worker/platform.buri
# from "platform/effect" import { Request, Response };
# from "platform/host" import { HostAllocator };
# from "//platform/effect/kv" import { Kv };
#
# export struct CloudflareHost {
#     export alloc: HostAllocator,
#     export kv: HostKv,
# }
#
# export fn fetch(host: CloudflareHost, request: Request): Response;
#
struct HostKv {}

impl Kv for HostKv {
    fn get(self, namespace: Str, key: Str): Option<Str>;

    fn put(self, namespace: Str, key: Str, value: Str): ();
}
```

```js
export const HostKv = {
  get: async (self, namespace, key) => (await bindings[namespace].get(key)) ?? undefined,
  put: (self, namespace, key, value) => bindings[namespace].put(key, value),
};
```

The build checks this, so a missing method fails `buri build` rather than the
first request that calls it.
