---
title: Only a `JS` entry offers an effect its platform implements
message: '`{field}` is a `{struct}`, which `{platform}` declares itself, and a `NATIVE` entry has no `js` file to implement it'
note: a platform implements an effect the backend lacks in its entry's `js` file, and only a `JS` entry has one
fix: build the entry with `backend: JS`, or drop the field and ship ordinary functions over the bundled effects
reproduction: none
---
# Only a `JS` entry offers an effect its platform implements

```text
error: `kv` is a `HostKv`, which `//platform/lambda` declares itself, and a `NATIVE` entry has no `js` file to implement it [custom-effect-on-native-backend]
```

A native platform offers the bundled effects only. It ships functions over them
instead, tested with `platform/effect/testing` like any other code:

```buri
export struct LambdaHost {
    export alloc: HostAllocator,
    export net: HostNetwork,
    export env: HostEnvironment,
}

export fn next<C: Allocator + Network + Environment>(ctx: C): Result<Str, Str> {
    .Err("not yet")
}
```
