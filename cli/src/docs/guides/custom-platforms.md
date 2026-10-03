# Write your own platform

A platform is where a program runs: what calls it, and what it may do there.
This guide writes a Cloudflare Worker with KV storage, which no bundled platform
has. The finished repository is
`cli/tests/repositories/custom-platforms/cloudflare_kv/repo`.

```text
platform/effect/kv/          the Kv effect, its wrappers, and TestKv
platform/cloudflare_worker/  the platform: CloudflareHost, HostKv, fetch.mjs
lib/sessions/                a library that reads sessions from any Kv
cmd/site/                    the worker
```

## The effect

KV is something a program does to the world, so it's an effect. Effects live in
packages under `platform/effect/`, apart from any platform, so another platform
could offer `Kv` too.

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_kv/repo package=//platform/effect/kv
export effect Kv {
    fn get(self, namespace: Str, key: Str): Option<Str>;
    fn put(self, namespace: Str, key: Str, value: Str): ();
}

export fn get<C: Kv>(ctx: C, namespace: Str, key: Str): Option<Str> {
    ctx.get(namespace, key)
}

export fn put<C: Kv>(ctx: C, namespace: Str, key: Str, value: Str): () {
    ctx.put(namespace, key, value)
}
```

Every effect ships a test implementation in its package's `testing` surface. It
keeps its store in `core/platforms/testing/state`, so each `kv()` starts empty:

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_kv/repo package=//platform/effect/kv role=testing
from "core/map" import * as map;
from "core/map" import { Map };
from "core/platforms/testing/state" import * as state;
from "//platform/effect/kv" import { Kv };

export struct TestKv {
    store: state.State<Map<(Str, Str), Str>>,
}

impl Kv for TestKv {
    fn get(self, namespace: Str, key: Str): Option<Str> {
        state.read(self.store).get((namespace, key))
    }

    fn put(self, namespace: Str, key: Str, value: Str): () {
        state.update(self.store, fn(c, m) => (m.insert(c, (namespace, key), value), ()))
    }
}

export fn kv(): TestKv {
    TestKv { store: state.new(map.empty()) }
}
```

```textproto schema=build
# platform/effect/kv/BUILD.buri
library {
    visibility: ["//visibility:public"]

    testing {}
}
```

## The platform

A platform rule names its entries and the backend that builds each:

```textproto schema=build
# platform/cloudflare_worker/BUILD.buri
platform {
    dependencies: ["//platform/effect/kv"]

    entry {
        name: "fetch"
        backend: JS
        js: "fetch.mjs"
    }
}
```

Its `platform.buri` declares the host type, one field per thing a worker may
do, and the entry without a body. `HostKv` is the platform's own production
struct, and its methods have no body either:

```buri ignore why="a platform's surface, compiled only with its rule"
from "platform/host" import { HostAllocator, HostClock, HostNetwork, HostStdout };
from "//platform/effect/kv" import { Kv };
from "platform/effect" import { Request, Response };

export struct CloudflareHost {
    export alloc: HostAllocator,
    export stdout: HostStdout,
    export clock: HostClock,
    export net: HostNetwork,
    export kv: HostKv,
}

export fn fetch(host: CloudflareHost, request: Request): Response;

struct HostKv {}
impl Kv for HostKv {
    fn get(self, namespace: Str, key: Str): Option<Str>;    // fetch.mjs implements both
    fn put(self, namespace: Str, key: Str, value: Str): ();
}
```

`fetch.mjs` adapts the worker runtime's call and implements `HostKv`. It gets
the compiled entry from `buri:program`:

```js
import { fetch } from "buri:program";
let bindings = {};

export default { fetch(request, env) { bindings = env; return fetch(request); } };
export const HostKv = {
  get: async (self, namespace, key) => (await bindings[namespace].get(key)) ?? undefined,
  put: (self, namespace, key, value) => bindings[namespace].put(key, value),
};
```

The build checks `HostKv` has `get` and `put`, with three and four parameters,
and awaits every call to them. A `Request` arrives as the Fetch standard's, and
the `Response` leaves as one. [The crossing table](../reference/build/platforms.md#the-crossing-table)
says how every other type crosses.

## The program

A library bounds its context by the effect and calls the wrappers. It never
sees `HostKv` or `TestKv`:

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_kv/repo package=//lib/sessions
from "core/str" import * as str;
from "platform/effect" import { Allocator };
from "//platform/effect/kv" import * as kv;
from "//platform/effect/kv" import { Kv };

export fn user<C: Allocator + Kv>(ctx: C, token: Str): Option<Str> {
    kv.get(ctx, "SESSIONS", str.format(ctx, "session:${token}"))
}
```

The worker's entry takes the host, binds what it needs, and delegates:

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_kv/repo package=//cmd/site role=entry
from "core/net/http" import * as http;
from "core/str" import * as str;
from "platform/effect" import { Allocator, Request, Response };
from "//lib/sessions" import * as sessions;
from "//platform/cloudflare_worker" import { CloudflareHost };
from "//platform/effect/kv" import { Kv };

export fn fetch(host: CloudflareHost, request: Request): Response {
    handle(
        context {
            Allocator: host.alloc,
            Kv: host.kv,
        },
        request,
    )
}

export fn handle<C: Allocator + Kv>(ctx: C, request: Request): Response {
    match (sessions.user(ctx, request.header("x-session").withDefault(""))) {
        .Some(name) => http.text(ctx, str.format(ctx, "hello ${name}")),
        .None => http.status(401),
    }
}
```

```textproto schema=build
# cmd/site/BUILD.buri
binary {
    dependencies: ["//lib/sessions", "//platform/effect/kv"]
    outputs: [
        { platform: "//platform/cloudflare_worker" },
    ]

    test {
        sources: ["test/handle_test.buri"]
        dependencies: ["//platform/effect/kv/testing"]
        backends: [NATIVE, JS]
    }
}
```

A test calls `handle` with test implementations, on either backend:

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_kv/repo package=//cmd/site role=test
from "core/testing/assert" import * as assert;
from "platform/effect" import { Allocator, Header, Request };
from "platform/effect/testing" import { alloc };
from "//cmd/site/main.buri" import { handle };
from "//platform/effect/kv" import * as store;
from "//platform/effect/kv" import { Kv };
from "//platform/effect/kv/testing" import { kv };

fn request(token: Str): Request {
    Request {
        method: .Get,
        url: "https://site.example/",
        headers: [Header { name: "x-session", value: token }],
        body: [],
        timeoutMillis: 0,
    }
}

test "an unknown token is refused" {
    assert.equal(
        handle(
            context {
                Allocator: alloc(),
                Kv: kv(),
            },
            request("t1"),
        ).status,
        401,
    );
}

test "a known token is greeted" {
    let ctx = context {
        Allocator: alloc(),
        Kv: kv(),
    };
    store.put(ctx, "SESSIONS", "session:t2", "ada");
    assert.equal(handle(ctx, request("t2")).status, 200);
}
```

## Build it

```sh
buri test //...
buri build //cmd/site
```

The worker lands at `.buri/out/platform/cloudflare_worker/cmd/site/fetch.mjs`,
ready for the worker runtime. `buri run` refuses it: a worker is called, not
started. Editing `fetch.mjs` or `platform.buri` rebuilds it, no `buri clean`
needed.

## Vars and secrets

A worker's vars and secrets arrive in the `env` the runtime passes beside each
request. Reading them is one more effect, `Vars`:

```buri repo=cli/tests/repositories/custom-platforms/cloudflare_worker/repo package=//platform/effect/vars
/// A worker's vars and secrets: the bindings its runtime hands each request
/// whose value is a string.
export effect Vars {
    fn get(self, name: Str): Option<Str>;
    fn all(self): [(Str, Str)];
}

export fn get<C: Vars>(ctx: C, name: Str): Option<Str> {
    ctx.get(name)
}

export fn all<C: Vars>(ctx: C): [(Str, Str)] {
    ctx.all()
}
```

The platform adds `vars: HostVars` to `CloudflareHost`, and `fetch.mjs` keeps
the `env` and implements it. A KV namespace is a binding too, but not a string,
so it's no variable:

```js
let bindings = {};

export default {
  fetch(request, env) {
    bindings = env ?? {};
    return fetch(request);
  },
};

export const HostVars = {
  get: (self, name) => (typeof bindings[name] === "string" ? bindings[name] : undefined),
  all: (self) => Object.entries(bindings).filter(([, value]) => typeof value === "string"),
};
```

A test binds `TestVars` from `//platform/effect/vars/testing` instead. The
repository is `cli/tests/repositories/custom-platforms/cloudflare_worker/repo`.

## What a platform can't do

- **A native platform offers bundled effects only.** A method without a body
  needs a `js` file, and only a `JS` entry has one. Ship ordinary functions over
  the bundled effects instead.
- **A platform can't add a backend, linker flags or native libraries.** Those
  are the toolchain's.
- **Libraries need nothing.** One bounded by `C: Kv` compiles anywhere and runs
  wherever an entry can bind a `Kv`.

The rules in full are in [platforms](../reference/build/platforms.md).
