---
title: Only a `JS` entry has a `js` file
message: '`{entry}` is a `NATIVE` entry with a `js` file'
label: a `js` file here
note: a `js` file is how a host calls a `JS` entry, and a `NATIVE` entry starts itself
fix: 'remove `js`, or build the entry with `backend: JS`'
reproduction: none
---
# Only a `JS` entry has a `js` file

```text
error: `bootstrap` is a `NATIVE` entry with a `js` file [js-outside-js-entry]
```

```textproto schema=build
# platform/lambda/BUILD.buri
platform {
    entry {
        name: "bootstrap"
        backend: NATIVE
    }
}
```
