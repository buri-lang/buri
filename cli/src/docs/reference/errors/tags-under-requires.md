---
title: A required tag would be forced onto every library
message: '`requires` takes no `tags`'
note: carrying no tags is the common case, so requiring a tag transitively would force it onto every library; what this usually means is `forbids {{ tags: [...] }}`
fix: what this usually means is `forbids {{ tags: [...] }}`
reproduction: none
---
