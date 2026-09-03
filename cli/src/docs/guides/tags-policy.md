# Enforce policy with tags

Say a repository has code that opens sockets and code that runs untrusted input,
and the two may never end up in one artifact. Tags are how that is written down
and checked.

## Declare the vocabulary

Every tag is declared once, in `REPO.buri`, with what it costs:

```textproto schema=repo
tag {
    name: "net"
    doc: "opens sockets"

    requires {
        platforms: [LINUX, MACOS]
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

Write `doc` as the policy rather than a restatement of the name — it is printed
in the diagnostic, and by then the reader wants to know why.

`forbids` is symmetric, so declaring it on one of the pair is the whole
statement. Put it on the restricted side, which is where somebody will look.

## Label the targets

A build file says what its code *is*, and nothing about what follows:

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
        { platform: MACOS, arch: ARM64 },
    ]
}
```

Adding a library that reuses an existing tag never touches `REPO.buri`, and
changing what `net` means never touches a library.

A tag `REPO.buri` does not declare is `unknown-tag`, with the nearest declared
name suggested — so a typo cannot quietly turn a checked build into an unchecked
one.

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

The path is printed because the interesting question is never "which library is
tagged `net`" but who dragged it in. The check runs at every target, not only at
binaries, so an unsatisfiable library is reported at itself rather than at
whichever binary reaches it first.

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

`requires { platforms: [...] }` is a whitelist, and it accumulates down the
closure. `net` requiring Linux and macOS above means every library tagged `net`
inherits that, and a binary asking for a JavaScript output fails a second way:

```text
error: //apps/scan cannot be built for js [platform-violation]
  = //libs/socket is tagged "net", which requires linux, macos
  = reached by: //apps/scan -> //libs/socket
  = "net": opens sockets
  = fix: drop the js output, or widen the tag's `requires { platforms }` in REPO.buri
```

That is the reason to state a deployment policy on the tag: one declaration,
enforced from both ends. `buri query 'platforms(//apps/scan)'` prints what the
closure has left.

---

Tags answer "what may end up in one artifact." For "who may write this
dependency edge," use `visibility`. Exact semantics — the closure union, the
platform intersection, and why `forbids` has no platforms — are in
[`tags.md`](../reference/build/tags.md).
