---
title: A generated rule places a source where the imports already put it
message: '{source} is reachable from {reached}, so `gen` cannot say which rule it belongs to'
note: guessing would move code across a boundary that exists to be explicit
fix: add `{source}` to one rule's `{field}`
reproduction: none
---
