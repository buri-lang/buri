# Enforce policy with tags

Say code that opens sockets and code that runs untrusted input must never end up
in one artifact. Tags state that rule, and the build checks it.

## Declare the vocabulary

Declare every tag once, in `REPO.buri`:

```textproto schema=repo
tag {
    name: "net"
    doc: "opens sockets"

    requires {
        backends: [NATIVE]
    }
}

tag {
    name: "sandboxed"
    doc: "runs untrusted input and may not reach the network"

    forbids {
        tags: ["net"]
    }
}
```

Write `doc` as the policy, not a restatement of the name: the diagnostic prints
it. `forbids` is symmetric, so declare it once, on the restricted side.

## Label the targets

A build file says what its code *is*:

```textproto schema=build
# libs/socket/BUILD.buri
library {
    sources: ["socket.buri"]
    tags: ["net"]
    visibility: ["//visibility:public"]
}
```

```textproto schema=build
# apps/scan/BUILD.buri
binary {
    dependencies: ["//libs/socket"]
    tags: ["sandboxed"]

    outputs: [
        { platform: "native", variant: "macos-arm64" },
    ]
}
```

An undeclared tag is an `unknown-tag` error that suggests the nearest declared
name, so a typo can't silently skip the check.

## Watch it fail

```text
$ buri build //apps/scan
error: //apps/scan cannot contain both "net" and "sandboxed" code [tag-violation]
 --> apps/scan/BUILD.buri:3:12
  |
3 |     tags: ["sandboxed"]
  |            ^^^^^^^^^^^
  |
  = "net" is carried by //libs/socket
        reached by: //apps/scan -> //libs/socket
  = "sandboxed" is carried by //apps/scan itself
  = "net": opens sockets
  = "sandboxed": runs untrusted input and may not reach the network
  = fix: drop one of the two dependencies, or split //apps/scan into a target per side
```

The error prints the path because the real question is who dragged `net` in. The
check runs at every target, not only binaries, so an unsatisfiable library is
reported at itself.

## Ask before you build

`buri query` answers the same questions without compiling anything:

```text
$ buri query 'tags(//apps/scan)'
net  (//libs/socket)
sandboxed  (//apps/scan)

$ buri query 'path(//apps/scan, //libs/socket)'
//apps/scan
  -> //libs/socket          apps/scan/BUILD.buri:2
```

## Restrict a tag to platforms

`requires { backends: [...] }` is an allowlist that accumulates down the closure.
`net` requires `NATIVE`, so a binary that depends on `net` code and asks for a
`node` output fails a second way:

```text
error: //apps/scan cannot be built for node [platform-violation]
  = //libs/socket is tagged "net", which requires backends NATIVE
  = reached by: //apps/scan -> //libs/socket
  = "net": opens sockets
  = fix: drop the node output, or widen the tag's `requires` in REPO.buri
```

`buri query 'platforms(//apps/scan)'` prints what the closure has left.

To rule out one platform and keep the rest open, including future ones, use
`forbids { backends: [JS] }` or `forbids { platforms: ["web"] }` instead.

---

Tags answer "what may end up in one artifact." For "who may write this
dependency edge," use `visibility`. [`tags.md`](../reference/build/tags.md) has
the exact semantics: the closure union, the platform intersection, and when to
forbid a platform rather than allowlist the rest.
