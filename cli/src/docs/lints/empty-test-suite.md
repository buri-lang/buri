---
title: A `test` block declares the sources it tests
severity: warning
message: this {rule}'s `test` block declares no sources
note: an empty suite reads as coverage to anything that walks the build graph
fix: list the suite's files in `test {{ sources }}`, or drop the empty block
---
