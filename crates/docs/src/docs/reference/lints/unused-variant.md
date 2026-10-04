---
title: Every variant is constructed or matched
severity: warning
message: "nothing constructs or matches `{type}.{name}`"
note: a variant nothing builds is a case the value can never be in, and every `match` on the enum still carries an arm for it
fix: delete the variant, or construct it
---
Every reader has to work out what could put a value in an unreachable case.

- A `_` arm keeps no variant alive, since it says you don't care which case
  occurs.
- Naming the variant in a pattern does keep it alive, even if nothing constructs
  it. Whoever wrote that arm thought the case could happen; check who's right
  before deleting.
- A `derive` keeps no variant alive.

Enums on a surface (`lib.buri`, or `testing/lib.buri` for a fixture) are never
reported, since an unseen consumer may build any case.
