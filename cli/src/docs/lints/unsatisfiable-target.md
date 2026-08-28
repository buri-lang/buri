---
title: A target admits at least one platform
severity: warning
message: {target} can never be built
note: its dependency closure admits no platform at all
fix: widen a tag's `requires {{ platforms }}` in REPO.buri, or drop the dependency that narrows it to nothing
---
