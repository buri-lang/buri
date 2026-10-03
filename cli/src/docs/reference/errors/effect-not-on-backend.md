---
title: A host offers only what its backend implements
message: '`{field}` is a `{struct}`, and the {backend} backend has no implementation of {effects}'
note: '`JS` lacks `Listen` and `Tcp`, and `NATIVE` lacks `Ui` and `Watch`'
fix: drop the field from `{platform}`'s host, or build the entry with the other backend
reproduction: none
---
# A host offers only what its backend implements

```text
error: `listen` is a `HostListen`, and the JS backend has no implementation of `Listen` [effect-not-on-backend]
```

`JS` holds no port open and dials no raw socket. `NATIVE` runs the reactive
graph only for tests, so it has no `Ui` or `Watch` to hand an entry.
