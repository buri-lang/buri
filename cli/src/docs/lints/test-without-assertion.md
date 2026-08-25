---
title: A test asserts something
severity: warning
message: test {title} asserts nothing
note: nothing reachable from this test calls `core/testing/assert`, so it passes as long as it does not abort
fix: assert what the test is for, or delete it
---
