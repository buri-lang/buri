---
title: An entry point is named by its rule, never listed
message: {source} is named by the rule, not listed
note: an entry point is named by the rule kind — lib.buri for a library, main.buri for a binary, testing/lib.buri for a `testing` block — rather than listed among its inputs
fix: remove "{source}" from the list; the rule already names it
reproduction: none
---
