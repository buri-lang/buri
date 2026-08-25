---
title: Modules form a graph with no cycles
message: 'circular import: {cycle} -> {path}'
note: modules form a graph with no cycles, at the module level and at the package level alike
fix: 'break the cycle: move what both modules need into a third one'
reproduction: none
---
