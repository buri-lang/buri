---
title: A document needs a JavaScript build
message: a native build has no document for {operations}
note: the document half of `ui` is JavaScript's, which is `ui/web` and `ui/node`'s `mount`
fix: build this for the `web` platform or give the suite `backends: [JS]`, and render a tree in a native test with `platform/effect/testing`'s `render`
reproduction: none
---
# A document needs a JavaScript build

```text
error: a native build has no document for `ui_web.render` [document-unavailable]
```

A native artifact has nothing to render into. A test that checks what a tree
shows can stay native: `platform/effect/testing`'s `render` answers the tree
headless, and a test presses, follows and reads it from there.
