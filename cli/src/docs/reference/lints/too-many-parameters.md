---
title: A function takes too many parameters
severity: warning
message: "`{name}` takes {count} parameters"
note: "`self` and `ctx` are not counted, so {limit} is the limit on the data a caller has to assemble"
fix: group the parameters that always travel together into a struct, and take that instead
adapted-from: habit-hooks (https://github.com/habit-hooks/habit-hooks) guides/too-many-parameters.md, © 2026 Ivett Ördög, used under the MIT license
---
Parameters that travel together across calls are a missing abstraction. To find
it:

- Look at the call sites and nearby functions for a type a group of these
  parameters already belongs to. Values that keep appearing side by side are
  usually one of the domain's nouns.
- If there's none, create it, and move the behaviour that uses those fields onto
  it.
- If one value owns most of the parameters, the function may belong on it, or
  should take it instead.
- Use the new type everywhere it fits, not just here. A call passing three of
  its fields is the same concept under the threshold.

Try rewriting each call site with the signature that feels natural there, and
let that shape the function.

**Avoid** a `{ ...everything }` bag that just renames the list. A `FooProps`
named after its function is the same bag, so the next function invents another
and the concept stays unnamed. You're done when the type has a domain name and
no call site passes its fields loose.
