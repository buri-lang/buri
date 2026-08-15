## What it does

Builds the targets you name together with their `test.sources`, runs every
`test` declaration in them, and reports one line per failure and a summary.

A test builds its own context, so a suite decides for itself what the code
under test is allowed to do. That is the whole of the mocking story: a test
double is a struct with methods, bound in a context the way the platform's
implementations are bound in `main`.

Exit status is `0` when every test passed and `1` when any did not — so `buri
test` is usable directly as a gate.
