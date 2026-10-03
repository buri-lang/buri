---
title: A tag never requires and forbids the same platform
message: tag "{tag}" both requires and forbids `{platform}`
note: '`requires` admits `{platform}` and `forbids` rules it out, so one of the two is a mistake'
fix: keep `{platform}` under `requires` or under `forbids`, not both
reproduction: none
---
