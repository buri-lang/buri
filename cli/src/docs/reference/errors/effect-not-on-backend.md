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

A host's fields are production values its entry's backend supplies. Both
backends implement every bundled effect, except that `JS` holds no port open and
dials no raw socket, and `NATIVE` runs the reactive graph only for a test, so it
has no `Ui` or `Watch` to hand an entry. `Location` is no backend's: `web`
implements its own.
