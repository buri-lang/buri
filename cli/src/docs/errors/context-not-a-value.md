---
title: A context declaration is called, not named
message: '`{name}` is a context; construct one by calling it'
note: each call builds a fresh context, so two tests never share one's state
fix: write `{name}()`
reproduction: none
---
