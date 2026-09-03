---
title: A suite finishes inside its `timeout_seconds`
message: "{target}'s test suite ran longer than {seconds}s"
label: the timeout this suite declares
note: the run was killed, so no test in this suite has a result — a timeout is the suite's, not one test's
fix: raise `timeout_seconds`, or find the test that does not finish; a test that never returns is a loop with no exit, since nothing here blocks on I/O
reproduction: none
---
