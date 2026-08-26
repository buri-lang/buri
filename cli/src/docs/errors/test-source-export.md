---
title: A test source exports nothing
message: a test source may not export
note: test sources are compiled independently and are not modules anybody can name; shared helpers belong in a library
fix: drop the `export`; move the declaration into a library if something else needs it
reproduction: none
---
