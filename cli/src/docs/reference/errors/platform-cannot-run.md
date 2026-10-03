---
title: '`buri run` runs an entry that starts itself'
message: '{target} builds `{entry}` for `{platform}`, and its `js` file calls it, so there is nothing to start'
note: '`buri run` starts an entry with the program signature, `fn(host: H): Result<(), Str>`, with no `js` file'
fix: 'build it with `buri build {target}`, and let the platform call it'
reproduction: none
---
# `buri run` runs an entry that starts itself

```text
error: //cmd/site builds `fetch` for `//platform/cloudflare_worker`, and its `js` file calls it, so there is nothing to start [platform-cannot-run]
```

An entry with a `js` file is called by whatever loads that file: a worker
runtime, a browser extension, a test script. Build it and hand the output
directory to that host.

```sh
buri build //cmd/site
bun -e 'import w from "./.buri/out/platform/cloudflare_worker/cmd/site/fetch.mjs"; ...'
```
