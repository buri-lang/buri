---
title: A target is built only for a platform its closure admits
message: '{target} cannot be built for {platform}'
fix: drop the {platform} output, or widen the tag's `requires {{ platforms }}` in REPO.buri
reproduction: none
---
