---
title: A chain has a bounded length
message: this chain is too long
fix: split it with `let` bindings; the limit exists so that a pathological input cannot exhaust the stack of the passes that walk what this builds
reproduction: none
---

A chain is `a + b + c`, `x.f().g()` or a run of `else if`. The limit is
2,048 links, far more than anyone writes by hand.
