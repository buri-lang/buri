---
title: A function takes too many parameters
severity: warning
message: "`{name}` takes {count} parameters"
note: "`self` and `ctx` are not counted, so {limit} is the limit on the data a caller has to assemble"
fix: group the parameters that always travel together into a struct, and take that instead
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/too-many-parameters.md, © 2026 Ivett Ördög, used under the MIT license
---
High parameter count is a sign of coupling.
Parameters that travel together across several calls are a missing abstraction.

**Find the missing abstraction:**
1. Look at the call sites and nearby functions — is there an existing class a group of these parameters belongs to? Search wider than the file that fired: values that keep appearing side by side are the entity, and it is usually one of the domain's own nouns — where that name already exists, it is the answer.
2. If not, create it — then move behavior that uses those fields onto it.
3. If one object owns most of the parameters, it may be the natural home for this function or perhaps the function should accept that object instead.
4. Use it at every site it fits, not only the one that fired — a call passing three of its fields is the same concept sitting under the threshold.

Useful tip: rewrite each call site with the signature that feels natural there, and let that shape the final method. 

**AVOID**: A `{ ...everything }` bag that merely renames the list hides the coupling instead of removing it. A `FooProps` or options object named after the function that takes it is the same bag: organised by method rather than by abstraction, so the next function invents another one and the concept stays unnamed. You are done when the entity carries a domain name and no call site still passes its fields loose.
