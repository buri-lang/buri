---
title: `requires` holds no `tags`
message: '`requires` takes no `tags`'
note: carrying no tags is the common case, so requiring a tag transitively would force it onto every library
fix: you probably mean `forbids {{ tags: [...] }}`
reproduction: none
---
