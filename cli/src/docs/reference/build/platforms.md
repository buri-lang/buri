# Platforms

Every output targets one platform. `native`, `node` and `web` come with the
toolchain. A repository writes its own as a `platform` rule under `platform/`,
and an output names it by label:

```textproto schema=build
binary {
    outputs: [
        { platform: "//platform/cloudflare_worker" },
    ]
}
```

## The platform rule

```textproto schema=build
platform {
    dependencies: ["//platform/effect/kv"]

    entry {
        name: "fetch"
        backend: JS
        js: "fetch.mjs"
    }
    assets: ["wrangler.toml"]
}
```

- **Location.** Under `platform/`, outside `platform/effect/`. Anywhere else is
  `rule-outside-its-directory`.
- **`entry`** names one way in, its `backend` (`NATIVE` or `JS`), and on `JS` an
  optional `js` file. An output builds every entry, each to a file named after
  it: `fetch.mjs`, or a native `bootstrap`.
- **An entry's `variants`** are names an output picks between with `variant`,
  and `variant_required: true` makes every output pick one. A `NATIVE` entry
  reads its target from the variant, as `linux-arm64`, and builds the host's
  without one.
- **`assets`** are files copied beside every output.
- **`sources`** and **`dependencies`** are the rule's other `.buri` files and
  the libraries `platform.buri` imports. An output's platform is a dependency of
  its binary without being listed.

## `platform.buri`

The platform's surface: its host type, its entries without bodies, and its own
production structs.

```text
export struct CloudflareHost { export alloc: HostAllocator, export kv: HostKv }
export fn fetch(host: CloudflareHost, request: Request): Response;

struct HostKv {}
impl Kv for HostKv { fn get(self, namespace: Str, key: Str): Option<Str>; ... }
```

The whole file is in [the guide](../../guides/custom-platforms.md#the-platform).

- **The host type** lists what the platform offers, one production struct per
  field. The CLI builds the value and hands it to the entry.
- **A consumer's entry** has the declaration's name and signature. Another host
  is `entry-host-mismatch`, and no host is `entry-without-host`.
- **A field the backend lacks** is `effect-not-on-backend`: `JS` has no `Listen`
  or `Tcp`, and `NATIVE` no `Location`.
- **The platform's own structs** have bodiless methods, which the entry's `js`
  file implements. Only `JS` entries have one, so on `NATIVE` such a field is
  `custom-effect-on-native-backend`.
- Only `platform.buri` may import `platform/host` or declare a function without
  a body.

## The `js` file

The CLI bundles each entry's `js` file with the compiled program into one
module, written at the entry's name. The file imports the entry from
`buri:program`:

```js
import { fetch } from "buri:program";
let bindings = {};

export default { fetch(request, env) { bindings = env; return fetch(request); } };
export const HostKv = {
  get: async (self, namespace, key) => (await bindings[namespace].get(key)) ?? undefined,
  put: (self, namespace, key, value) => bindings[namespace].put(key, value),
};
```

- **An export named after a production struct implements it.** The build reads
  `export const HostKv = { ... }` and `export function`, and checks every method
  is there with the same parameter count, `self` included. A gap is
  `host-file-incomplete`.
- **Every other export is the module's own**, such as `default` here.
- **Every call to a method the file implements is awaited**, so a method may
  return a promise.
- An entry without a `js` file starts itself, so it has the program signature,
  `fn(host: H): Result<(), Str>`, and `buri run` runs it. One with a `js` file
  is `platform-cannot-run`.

## The crossing table

What an entry with a `js` file takes and answers, and what a method the file
implements takes and answers, crosses to JavaScript like this:

| Buri                    | JavaScript                     |
|---|---|
| `Str`, `Bool`, `F64`    | `string`, `boolean`, `number`  |
| `Int`, other integers   | `bigint`                       |
| `[U8]`                  | `Uint8Array`                   |
| `[T]`, a tuple          | `Array`                        |
| `Option<T>`, `()`       | the value, or `undefined`      |
| `Result<T, Str>`        | the value, or a thrown `Error` |
| a struct                | a plain object of its fields   |
| `Request`, `Response`   | the Fetch standard's           |

`Result` crosses only as a whole answer, and `null` arrives as `None`. Anything
else is `type-cannot-cross`. A production struct's `self` arrives as `{}`.

## Effect packages

A custom effect lives in a library under `platform/effect/`, shared by every
platform:

```text
platform/effect/kv/lib.buri           effect Kv, and the functions get and put
platform/effect/kv/testing/lib.buri   TestKv, the test implementation
```

- **Declared anywhere else**, an effect is `effect-outside-effect-directory`.
- **Every effect has a test implementation** in the package's `testing`
  surface, or it's `effect-without-test-implementation`. Its state lives in
  `core/platforms/testing/state`, which only an effect's testing surface may
  import.
- **Code calls the wrapper functions**, `kv.get(ctx, ...)`. Only the package
  itself calls the effect's methods.

## Libraries and tags

A library's `platforms` and a tag's `requires` and `forbids` name a repository
platform by label:

```textproto schema=build
library {
    platforms: ["//platform/cloudflare_worker"]
}
```

That library goes only into the worker's outputs. `backends: [JS]` admits every
platform the `JS` backend builds, a repository's own included. A label that
names no `platform` rule is `no-such-platform`.

## Caching and layout

An output's key holds its platform's `BUILD.buri`, `platform.buri`, sources,
`js` files, assets and dependencies, so editing any of them rebuilds the outputs
that use it. Outputs land in `.buri/out/platform/<name>/<package>/`, with the
variant after the name when the output names one.
