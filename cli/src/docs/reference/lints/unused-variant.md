---
title: Every variant is constructed or matched
severity: warning
message: "nothing constructs or matches `{type}.{name}`"
note: a variant nothing builds is a case the value can never be in, and every `match` on the enum still carries an arm for it
fix: delete the variant, or construct it
---
An enum is a claim about the cases a value can be in. A variant nothing ever builds and no pattern ever names is a case the program cannot reach, and it is not free: every exhaustive `match` on the enum has an arm for it, and every reader has to work out what puts a value there.

A `_` arm names no variant, so an enum matched only by wildcard has nothing keeping its variants alive. That is deliberate — a wildcard is what a reader writes when they do not care which case it is, and it is the opposite of evidence that a particular case occurs.

Naming the variant in a pattern is enough on its own, even with nothing constructing it. Someone wrote the arm because they believe the case can happen, and the disagreement between that belief and the constructors is worth a look rather than a deletion.

A `derive` does not keep a variant alive, and the difference from a field is the question rather than an oversight: a derived fold reads every field's value, and it puts a value into no case at all.
