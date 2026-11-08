# Check sharing in test builds

`buri test` checks every suite the cache left to build, and the lint's targets, as one `driver::shared` compilation (`check_once` in `cli/src/commands/test.rs`, `design/PERFORMANCE.md` §6.104).

- `Checks::front` hands a clean share to `solo` and `queue_batch` as `Check::Shared`. A share with an error falls back to `Check::Own`, the old path.
- JavaScript suites restyle in parallel inside `check_once` (`Checks::fronts`). Native solo suites and batches restyle on the coordinator.
- Restyling is billed to check (`driver::Shared::restyled`). Build mode's `Shared::restyle` still bills it to other.
- Tests are numbered within the program (`monomorphize.rs`), so a bundle doesn't depend on what else was checked.
- Coverage runs skip it.

`monorepo` goals workspace, test build, after the change:

- check: 357 k lines/CPU s, 39.2 k instructions a line. Type checking is 3.9 G instructions; per-suite style graphs are 12.7 G (47 graphs).
- The one check runs on the coordinator, so checks no longer contend for the interner's shards. It wasn't measured further.

Next: the style graphs. Each UI suite draws its own `styles::Flow` over the same library code. Sharing the flow, or the part of it that doesn't depend on the program's callers, is what's between test-build check and 1 M lines/s.
