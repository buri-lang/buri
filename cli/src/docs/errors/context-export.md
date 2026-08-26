---
title: A `context` is exported only from a test-only module
message: a `context` may be exported only from a test-only module
note: a path containing a `testing` segment is importable only from a test source, which is what keeps the fixture out of a program
fix: drop the `export`, or move it into a test-only module
reproduction: none
---
