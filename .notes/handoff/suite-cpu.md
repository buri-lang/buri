# Suite CPU: what's left (2026-11-06)

Measured at `b06447ddb`, 12-core M3 Pro, load 9–19. Figures and method are in `design/PERFORMANCE.md` §6.100.

- Cold build of all test binaries: 807 CPU-s. A rebuild after touching `cli/src/lib.rs`: 245, about 150 of it the 13 integration-test crates.
- Test run: 1,134 CPU-s. `native` 402, `language` 319, `build` 202.
- Landed: `sweep::once_per_run` shares the unmodified conformance run across nextest processes (−80 CPU-s).

## Per-test CPU

Set `CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER` to a script that runs `/usr/bin/time -p -o <file>` around `"$@"` and logs the args beside it. Summing the files gives CPU by test.

## Next, largest first

- **Unicode tables on JavaScript, ~240 CPU-s a suite.** Eight JS conformance runs (3 in `coverage_agrees`, 2 in `release_and_debug_agree`, 2 in `formatting_the_corpus…`, 1 shared) each spend ~30 CPU-s in five tests: four in `unicode.buri` (decomposition, composition, folding, "a scalar written twice") and `characters.buri` "the classifiers count …". Each table lookup decodes base-36 digits with `BigInt` (`codeAt`/`digitAt` in `core/str` and `core/character`): `isPunctuation` is 10 µs a call on bun, `codeAt` 0.95 µs. Options: an `Int` fast path as `number`, or lookups that compare 4-char slices as strings (fixed-width base-36 sorts like the numbers).
- **Fold the nine small integration-test binaries into one, ~35 CPU-s per rebuild.** `adversarial`, `checking`, `ci`, `failing`, `formatting`, `fuzz`, `linting`, `recovery`, `vectors` each compile the 7.4k-line harness and link the toolchain. Touches nextest filters (`binary(checking)`), `ci.rs` and README tables.
- **`round_trip::every_conformance_source_survives_parse_and_print`, 77 CPU-s.** The `core/buri/ast` lexer and parser on bun, so the same `BigInt` cost.

## Measured and dropped

- Dependencies at `opt-level = 0`: build −88, run +19 CPU-s, and shared generics make the allocator test 8× slower.
- Dependencies without line tables: −13 CPU-s a cold build.
- Deduplicating the harness's 28 tests across 10 binaries: they cost 0.1 CPU-s in all.
