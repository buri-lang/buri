---
title: An effect has a test implementation beside it
message: '`{effect}` has no test implementation in `{package}/testing`'
note: an effect package ships a `testing` surface with a type that implements each of its effects
fix: 'give the library rule a `testing` block, and `impl {effect} for` a type in `testing/lib.buri`'
reproduction: none
---
# An effect has a test implementation beside it

```text
error: `Kv` has no test implementation in `//platform/effect/kv/testing` [effect-missing-test-impl]
```

A test plugs in a test implementation where production binds a host's field, so
an effect without one is an effect nobody can test. Keep its state in
`core/platforms/testing/state`, which only an effect's testing surface may
import:

```buri ignore why="an effect package's testing surface, compiled only in its package"
from "core/map" import * as map;
from "core/map" import { Map };
from "core/platforms/testing/state" import * as state;
from "//platform/effect/kv" import { Kv };

export struct TestKv {
    store: state.State<Map<Str, Str>>,
}

impl Kv for TestKv {
    fn get(self, namespace: Str, key: Str): Option<Str> {
        state.read(self.store).get("${namespace}/${key}")
    }

    fn put(self, namespace: Str, key: Str, value: Str): () {
        state.update(self.store, fn(c, m) => {
            (m.insert(c, "${namespace}/${key}", value), ())
        })
    }
}

export fn kv(): TestKv {
    TestKv { store: state.new(map.empty()) }
}
```
