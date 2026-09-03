---
title: A test source is not a module anybody can name
message: '{path} is a test source'
label: test sources are not importable
note: test sources are compiled independently and are not modules anybody can name
fix: put the shared helper in a library and list it in `test.dependencies` — a path with a `testing` segment if it is test-only
reproduction: none
---
