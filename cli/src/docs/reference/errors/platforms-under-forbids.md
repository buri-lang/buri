---
title: A platform restriction is a whitelist under `requires`
message: '`forbids` takes no `platforms`'
note: a platform restriction is always a whitelist under `requires`, so that adding a platform to the toolchain cannot silently widen code written before it existed
fix: move the list under `requires {{ platforms: [...] }}`
reproduction: none
---
