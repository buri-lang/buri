# Outputs, configurations, and tags

One mechanism answers two questions that look different and are not: *which
platforms can this code be built for*, and *is this code allowed to end up in
that program*.

**Tags mean the same thing on a library and on a binary.** There is no second
mechanism for entry points: a tag is always a statement about which
configurations a target accepts, and a binary is a target.

## Dimensions

A **dimension** is an axis a build varies along. `REPO.buri` declares them.
`platform` and `arch` are predeclared; the rest are yours.

```textproto
# REPO.buri

dimension {
  name: "tier"
  compose: INTERSECT
  value { name: "client" doc: "ships to a user's machine or browser" }
  value { name: "server" doc: "runs on infrastructure we operate" }
}
```

Value names are unique across all dimensions and tag sets, so `"server"` is
unambiguous and the qualified `"tier=server"` is only for emphasis. Both
spellings are accepted everywhere.

## Tags are constraints

A target's `tags` do not describe what it is. They restrict which configurations
it accepts:

```textproto
library {
  name: "store"
  tags: ["server"]        # only ever linked into a build whose tier is server
}
```

Read `tags: ["server"]` as *"the set of tier values I accept is {server}"*. A
dimension the target does not mention is unconstrained — it accepts every value.
So:

| `tags` | Accepts |
|---|---|
| *(empty)* | Every configuration. The common case. |
| `["server"]` | Any config with `tier=server`, any platform. |
| `["server", "linux"]` | `tier=server` **and** `platform=linux`. |
| `["linux", "macos"]` | `platform` is linux or macos — a native-only target. JS excluded. |

Two values of the *same* dimension are alternatives; values of *different*
dimensions are conjoined. There is no negation and no boolean syntax. "Anything
but JS" is written by listing what is allowed, which stays correct in the sense
that adding a platform to the repository does not silently widen a library
written before it existed.

### Tag sets

A combination used repeatedly gets a name, declared once:

```textproto
tag_set {
  name: "native"
  values: ["linux", "macos"]
  doc: "the platforms we ship a real binary for"
}
```

```textproto
library {
  name: "posix_paths"
  tags: ["native"]        # exactly as if it read ["linux", "macos"]
}
```

Expansion is textual and happens first, so a tag set may not contain another —
one level, no resolution order to reason about. A tag set may span dimensions,
which is how a deployment target ("`edge` = js + client") becomes one word.

## Composition: how a tag travels along an edge

Each dimension declares **how its tags compose** across a dependency edge. This
is the part that is user-definable, and it is per dimension because the two
useful behaviors are genuinely different questions.

```textproto
dimension { name: "tier"     compose: INTERSECT }
dimension { name: "maturity" compose: PROPAGATE }
dimension { name: "codegen"  compose: INDEPENDENT }
```

**`INTERSECT`** (the default) — *where may this go?* A target's effective
constraint is its own tags intersected with every dependency's effective
constraint:

```
effective(T) = tags(T) ∩ effective(d₁) ∩ effective(d₂) ∩ …
```

Restrictions accumulate downward: depending on server-only code makes you
server-only. This is what `tier` and `platform` want.

**`PROPAGATE`** — *what is in here?* A value carried by any dependency is
carried by the dependent, whether or not it asked for it. A target's own tags
are then the values it is **willing to carry**, and the check is that the union
travels up into a set the dependent allows:

```
carried(T) = tags-as-carried(T) ∪ carried(d₁) ∪ carried(d₂) ∪ …
```

This models licensing, data classification, and maturity — "does anything in
this binary touch PII", "is anything in here still experimental" — which
`INTERSECT` cannot express, because those are facts that spread rather than
permissions that narrow.

**`INDEPENDENT`** — no composition. Each target is checked against the
configuration on its own, and a dependency's tags say nothing about its
dependents. For axes that are about how a target is built rather than about
where it ends up.

## A configuration

A **configuration** assigns exactly one value to every `INTERSECT` dimension,
and a set of carried values to every `PROPAGATE` one. Resolving it, per output:

1. The `outputs` entry contributes `platform` and `arch`.
2. The binary's own `tags` narrow the rest.
3. Composition folds in every transitive dependency, by each dimension's rule.
4. An `INTERSECT` dimension still admitting more than one value takes the
   dimension's `default`, if it names one of them.
5. A dimension with nothing left, or with more than one value and no usable
   default, is an error.

```textproto
binary {
  name: "server"
  tags: ["server"]                          # narrows tier to {server}
  outputs: [
    { platform: LINUX, arch: X86_64 },      # config: linux, x86_64, server
    { platform: MACOS, arch: ARM64 },       # config: macos, arm64, server
  ]
}
```

Two outputs, two configurations, two independent builds of the whole graph.
`buri build //cmd/server` builds both; `--output=linux/x86_64` picks one.

A dimension with no `default` that nothing narrows is an error at the binary,
which is how you make an axis mandatory:

```
error: //cmd/web does not resolve dimension "tier"
  --> cmd/web/BUILD.buri:2:3
   |
 2 |   name: "web"
   |   ^^^^^^^^^^^
   |
   = nothing in this build narrows "tier", and it declares no default
   = values: client, server
   = add one to `tags`
```

## What a failure looks like

Take the [`example/`](./example/) repository, and suppose `//lib/ledger` grows a
dependency on `//lib/store` — a reasonable-looking edge, added by someone who
was not thinking about the browser build:

```
//cmd/web          tags: ["client"], outputs: [{ platform: JS }]
  └─ //lib/ledger  tags: []            <- the new edge is here
       └─ //lib/store  tags: ["server"]
```

```
error: //cmd/web cannot be built for js
  --> cmd/web/BUILD.buri:9:14
   |
 9 |   outputs: [{ platform: JS }]
   |             ^^^^^^^^^^^^^^^^
   |
   = configuration: platform=js, tier=client
   = //lib/store accepts only tier=server, and this build is tier=client
   = reached by: //cmd/web -> //lib/ledger -> //lib/store
   = the edge that introduces it: lib/ledger/BUILD.buri:9 deps "//lib/store"
   = "server": runs on infrastructure we operate
```

The path is printed because in a repository of any size the interesting question
is never "which library is tagged `server`" but "who dragged it in." The `doc`
string from `REPO.buri` is printed for the same reason: the tag is a policy, and
the policy should say why.

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

A `PROPAGATE` failure reads from the other direction — the binary refuses to
carry what a dependency brought:

```
error: //cmd/server carries maturity=experimental
  --> cmd/server/BUILD.buri:4:9
   |
 4 |   tags: ["stable"]
   |         ^^^^^^^^^^
   |
   = //lib/vector_index is tagged experimental, and "maturity" propagates
   = reached by: //cmd/server -> //lib/search -> //lib/vector_index
   = "experimental": API may change without a deprecation period
```

## Repository-wide policy

Dimensions and their composition rules constrain code. `constraint` blocks in
`REPO.buri` constrain *configurations* — combinations that should not exist at
all, independent of which targets are involved:

```textproto
constraint {
  when: "platform=js"
  require: ["tier=client"]
  message: "the JS output ships to browsers; server code is not shipped to users"
}

constraint {
  when: "public"
  forbid: ["experimental"]
  message: "public binaries link only code with a stable API"
}
```

Constraints are evaluated once per binary output, before the dependency graph is
walked, so the diagnostic is about the binary rather than about some library four
hops down:

```
error: //cmd/web has an invalid configuration
  --> cmd/web/BUILD.buri:4:9
   |
 4 |   tags: ["server"]
   |         ^^^^^^^^^^
   |
   = platform=js requires tier=client, but this binary narrows tier to server
   = the JS output ships to browsers; server code is not shipped to users
```

This is the "state in one place how a tag may be used" half of the design. A
target's tags say *which configurations it accepts*; a dimension's `compose`
says *how that travels*; a constraint says *which worlds may exist*. Adding a
library never requires editing `REPO.buri`, and changing policy never requires
editing a library.

## Tags and tests

A test suite runs in the configuration of the target under test. A target with
no tags is tested once, in the host configuration. A target tagged `server` is
tested in a server configuration. A suite that should run in more than one pins
them:

```textproto
library {
  name: "codec"
  sources: ["codec.buri"]

  test {
    sources: ["test/codec.buri"]
    # Run this suite once per platform; the JS run goes through the JS backend.
    tags: ["linux", "js"]
  }
}
```

That is the mechanism for "this must behave identically on both backends," which
for a language targeting a native binary and JavaScript is the test you most
want to be able to write. `I64` on the JS target ([`SPEC.md` §15](../SPEC.md))
is the standing reason it exists.

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
