# JavaScript suites run in one process

## What was found

`buri test` ran a JavaScript suite of more than 32 tests in up to three Bun
processes that pulled tests one at a time (`$pull`, `run_js_pulled`). Each
extra process loaded Bun, the runtime and the suite's modules again, and
compiled the hot code again.

Instructions retired by the Bun processes, measured with `/usr/bin/time -l`
through a `BURI_JS` wrapper:

| run | processes | instructions |
| --- | --- | --- |
| `cli/tests/conformance`, before | 85 | 113.3 G |
| `cli/tests/conformance`, after | 42 | 92.7 G |
| `a_long_javascript_suite` (60 tests), before | 2 | 640 M |
| `a_long_javascript_suite`, after | 1 | 325 M |

An empty Bun start is about 134 M instructions. Loading the 60-test bundle
takes about 320 M per process; the tests themselves take almost nothing.

## What changed

- `cli/src/commands/test.rs`: `start_js` always runs one process. The pulled
  mode (`JsPulled`, `run_js_member`, `run_js_pulled`, `Job::Js`) is gone.
- `crates/js/.../generate.rs`: `$pull` is gone from the test harness.
- Tests: `scheduling::a_long_javascript_suite_runs_in_one_process` guards the
  process count through `BURI_PROFILE`'s `test processes`. `test_corners` gained
  a hang and an output test for a long suite, and its runtime-dies test now
  kills the runtime with a signal. `agreement::row_14_a_literal_count_of_63_agrees`
  covers rotates and shifts by a literal 63.

## Open

Nothing. Native suites still spread over several processes; that wasn't in
scope.
