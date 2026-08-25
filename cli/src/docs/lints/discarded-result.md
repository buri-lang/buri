---
title: Every deliberately dropped `Result` is reported
severity: warning
message: this discards a `Result`
note: "`ignore` is the one way to drop a `Result`, so every place a failure is deliberately unhandled is one of these"
fix: handle the error with `match`, propagate it with `?`, or keep `ignore` if dropping it is deliberate
---
