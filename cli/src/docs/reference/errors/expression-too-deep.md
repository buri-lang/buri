---
title: An expression nests to a bounded depth
message: expression nests too deeply
fix: split it with `let` bindings; the limit exists so a pathological input cannot exhaust the parser's stack
reproduction: none
---
