---
title: A worker grants the effects `core/host` exports for it
message: '`{name}` implements {effect}, which is not allowed on {platforms}'
note: a platform is the set of effects its host exports; {because}
fix: drop {effect} from the context{elsewhere}
reproduction: none
---
# A worker grants the effects `core/host` exports for it

```text
error: `ui` implements `Ui`, which is not allowed on the CLOUDFLARE_WORKER platform [effect-not-on-platform]
```

## What to do

Drop the effect from the context, or build this target for a platform that
grants it.

The error lands on the import when the program imported the name directly, and
on the member reference when it came in as `* as host` — a namespace import
names no effect, so there is nothing to refuse until the program reaches for a
member.

## Where it fires

Only on a name from `core/host`, which only a `CLOUDFLARE_WORKER` entry's module
may import: a worker's `fetch(request: Request)` takes no host yet. Every other
entry takes its platform's host, and a field the host lacks is `no-such-field`
instead, with a note naming the platforms that offer it.

- **An entry's own body** is checked against the outputs that enter through
  *that* entry. So a binary whose page enters at `main` and whose worker enters
  at `fetch` may bind `Network: host.net` in `fetch`.
- **Anywhere else in `main.buri`** — a helper, a top-level named import — is
  checked against every platform the `outputs` name, because any of them may
  reach it. So a helper beside a worker's `fetch` that reads `host.ui` is
  refused, naming the worker.

An **effect type** is never platform-bound. `from "core/fs" import { FileSystemRead }`
is legal on every platform, and so is `platform/effect/testing`'s test
implementation of every effect.

## Why

A platform *is* the set of effects its host exports, so a worker that does not
grant an effect exports no name for it. The check needs only the build file and
the import line, both of which the editor has, so the language server reports it
as you type.

## A program that provokes it

It needs a build file naming a worker:
`cli/tests/repositories/build-files/several_entries` reads `host.ui` in a helper
beside a worker's `fetch`.
