---
title: A binary's entry point is imported only by its own tests
message: "{path} is a binary's entry point"
note: only that binary's own test sources may import it; a library may not reach the binary in its package at all
fix: move what you need into a library both can depend on
reproduction: none
---
