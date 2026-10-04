---
title: A target is built only for a platform its closure admits
message: '{target} cannot be built for {platform}'
fix: drop the {platform} output, or widen the list the note names
reproduction: none
---
# A target is built only for a platform its closure admits

```text
error: //cmd/app cannot be built for node [platform-violation]
```

Every library a binary reaches, and every tag those libraries carry, must admit
the platform each output names. The note says which member rules it out:
a library's own `backends` or `platforms`, or a tag's `requires` or `forbids`.
