# Outputs, configurations, and tags

One mechanism answers two questions that look different and are not: *which
platforms can this code be built for*, and *is this code allowed to end up in
that program*.

## Dimensions

A **dimension** is an axis a build varies along. `REPO.buri` declares them.
`platform` and `arch` are predeclared; the rest are yours.

```textproto
# REPO.buri

dimension {
  name: "tier"
  value { name: "client" doc: "ships to a user's machine or browser" }
  value { name: "server" doc: "runs on infrastructure we operate" }
}

dimension {
  name: "surface"
  value { name: "public" doc: "reachable by unauthenticated requests" }
  value { name: "internal" }
  default: "internal"
}
```

Value names are unique across all dimensions, so `"server"` is unambiguous and
the qualified `"tier=server"` is only for emphasis or disambiguation from a
same-named target. Both spellings are accepted everywhere.

## A configuration

A **configuration** assigns exactly one value to every dimension. Every build
action happens in one, and it comes from the binary:

- `platform` and `arch` come from the `outputs` entry being built.
- Everything else comes from the binary's `tags`, or from the dimension's
  `default`, or it is an error.

```textproto
binary {
  name: "server"
  tags: ["server"]                          # tier=server; surface defaults to internal
  outputs: [
    { platform: LINUX, arch: X86_64 },      # config: linux, x86_64, server, internal
    { platform: MACOS, arch: ARM64 },       # config: macos, arm64, server, internal
  ]
}
```

Two outputs, two configurations, two independent builds of the whole graph.
`buri build //cmd/server` builds both; `--output=linux/x86_64` picks one.

A dimension with no `default` that a binary does not pin is an error at the
binary, which is how you make an axis mandatory:

```
error: //cmd/web:web does not pin dimension "tier"
  --> cmd/web/BUILD.buri:2:3
   |
 2 |   name: "web"
   |   ^^^^^^^^^^^
   |
   = "tier" has no default in REPO.buri; every binary must choose
   = values: client, server
```

## Tags on a library are constraints

A library's `tags` do not describe what it is. They restrict where it may go:

```textproto
library {
  name: "store"
  tags: ["server"]        # only ever linked into a binary whose tier is server
}
```

Read `tags: ["server"]` as *"the set of tier values I accept is {server}"*.
A dimension the library does not mention is unconstrained — it accepts every
value. So:

| `tags` | Accepts |
|---|---|
| *(empty)* | Every configuration. The common case. |
| `["server"]` | Any config with `tier=server`, any platform, any surface. |
| `["server", "linux"]` | `tier=server` **and** `platform=linux`. |
| `["linux", "macos"]` | `platform` is linux or macos — a native-only library. JS excluded. |

Two values of the *same* dimension are alternatives; values of *different*
dimensions are conjoined. There is no negation and no boolean syntax. "Anything
but JS" is written by listing what is allowed, which stays correct in the sense
that adding a platform to the repository does not silently widen a library that
was written before it existed.

## Constraints compose up the graph

The **effective constraint** of a library is its own tags intersected with the
effective constraints of all its dependencies, per dimension:

```
effective(L) = tags(L) ∩ effective(d₁) ∩ effective(d₂) ∩ …
```

A binary's configuration must be accepted by the effective constraint of every
library it transitively reaches. Equivalently, and this is how the checker
actually runs: the config must be accepted by each library's own tags, checked
once per library.

The composition matters for diagnostics rather than for the result. Take the
[`example/`](./example/) repository, and suppose `//lib/ledger` grows a
dependency on `//lib/store` — a reasonable-looking edge, added by someone who
was not thinking about the browser build:

```
//cmd/web:web        tags: ["client"], outputs: [{ platform: JS }]
  └─ //lib/ledger    tags: []          <- the new edge is here
       └─ //lib/store  tags: ["server"]
```

```
error: //cmd/web:web cannot be built for js
  --> cmd/web/BUILD.buri:9:14
   |
 9 |   outputs: [{ platform: JS }]
   |             ^^^^^^^^^^^^^^^^
   |
   = configuration: platform=js, tier=client, surface=internal
   = //lib/store accepts only tier=server, and this build is tier=client
   = reached by: //cmd/web:web -> //lib/ledger -> //lib/store
   = the edge that introduces it: lib/ledger/BUILD.buri:9 deps "//lib/store"
   = "server": runs on infrastructure we operate
```

The path is printed because in a repository of any size the interesting
question is never "which library is tagged `server`" but "who dragged it in."
The `doc` string from `REPO.buri` is printed for the same reason: the tag is a
policy, and the policy should say why.

An empty intersection is an error at the library itself, before any binary asks
for it:

```
error: //lib/edge_cache can never be built
  --> lib/edge_cache/BUILD.buri:4:9
   |
 4 |   tags: ["client"]
   |         ^^^^^^^^^^
   |
   = it accepts only tier=client, but depends on //lib/store, which accepts
     only tier=server
```

Catching that at the library is worth the extra pass: otherwise the mistake
surfaces as a confusing failure in whichever binary happens to reach it first.

## Repository-wide policy

Dimensions constrain code. `constraint` blocks in `REPO.buri` constrain
*configurations* — combinations that should not exist at all, independent of
which libraries are involved:

```textproto
constraint {
  when: "platform=js"
  require: ["tier=client"]
  message: "the JS output ships to browsers; server code is not shipped to users"
}

constraint {
  when: "surface=public"
  forbid: ["internal"]
  message: "public-surface binaries link only reviewed, internal-free code"
}
```

Constraints are evaluated once per binary output, before the dependency graph
is walked, so the diagnostic is about the binary rather than about some library
four hops down:

```
error: //cmd/web:web has an invalid configuration
  --> cmd/web/BUILD.buri:4:9
   |
 4 |   tags: ["server"]
   |         ^^^^^^^^^^
   |
   = platform=js requires tier=client, but this binary pins tier=server
   = the JS output ships to browsers; server code is not shipped to users
```

This is the "state in one place how a tag may be used" half of the design. The
tag itself says *where this code may go*; the constraint says *which worlds may
exist*. Keeping them separate means adding a library never requires editing
`REPO.buri`, and changing policy never requires editing a library.

## Tags and tests

A test suite runs in the configuration of the target under test. A library with
no tags is tested once, in the host configuration. A library tagged `server` is
tested in a server configuration. A library whose tags admit several
configurations is tested in the host platform's, unless the suite pins others:

```textproto
library {
  name: "codec"
  srcs: ["codec.buri"]

  test {
    srcs: ["test/codec.buri"]
    # Run this suite once per platform; the JS run goes through the JS backend.
    tags: ["linux", "js"]
  }
}
```

That is the mechanism for "this must behave identically on both backends,"
which for a language targeting a native binary and JavaScript is the test you
most want to be able to write. `I64` on the JS target ([`SPEC.md`
§14](../SPEC.md)) is the standing reason it exists.

## What tags are not

- **Not a boolean expression language.** No `!`, no `or` across dimensions, no
  nesting. If a rule cannot be written as "these values, on these axes," it is
  probably a visibility rule.
- **Not conditional compilation.** No source file changes meaning across
  configurations; there is no `#if`. A library that needs two implementations
  gets two libraries with different tags and one dependent that picks — which is
  visible in the build graph rather than hidden in a file.
- **Not a substitute for visibility.** Visibility answers "who may write this
  dependency edge" and is checked one edge at a time. Tags answer "where may this
  code end up" and are checked transitively. Use visibility for API ownership,
  tags for deployment and platform boundaries.
