---
title: A build that selects no output skips the ones this host cannot build
severity: warning
message: skipped the `{selector}` output of {target}, which this host cannot build
fix: build it on a host that can, or run `buri build {target} --output={selector}` to see why
reproduction: none
---
# A build that selects no output skips the ones this host cannot build

```text
warning: skipped the `native/linux-x86_64` output of //cmd/server, which this host cannot build [output-unavailable]
```

A plain `buri build` builds every output this host can and skips the rest, so
a repository that ships a Linux server stays buildable on a Mac. Two outputs
can't be built everywhere:

- a macOS output, on a host that isn't a Mac;
- a Linux output that uses networking or cryptography, on a Mac.

`--output` names the output you want, so it fails instead of skipping.
