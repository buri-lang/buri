# Digest-pinned benchmark corpora

Manifests, and no source. Each file here names a profile, a parameter set and a
seed, and records the SHA-256 of the bytes that combination produced on the day
it was written. The harness regenerates the corpus on every run and checks the
digest **before it measures anything**. `design/PERFORMANCE.md` §3.1 states the
rule they exist under; this file is the operational half of it.

## Why a third kind

`cli/benches/corpora/` buys byte-stability by checking the bytes in, and it is
capped at 512 KiB per corpus for a reason: a million-line corpus is 35 MB, and
the whole repository's history is 15 MB. So the scale tier gets the same promise
by a different route — check in the *digest* rather than the bytes.

| Kind | Where | Buys | Costs |
|---|---|---|---|
| Generated | `generate.rs` | Any scale, any profile, no drift blindspot | Nothing pins the bytes across a generator change |
| Checked in | `corpora/` | The bytes themselves, reviewable in a diff | Megabytes in git; 512 KiB cap; 15k lines is the ceiling |
| **Digest-pinned** | **here** | **The bytes, at any scale, for 400 bytes of git** | **A mismatch is a failure, never a diff you can read** |

The last column is the honest cost. When a saved corpus moves, the diff shows
you *what* moved, in Buri. When a pinned corpus moves, all you get is two
hashes and the counts beside them — the manifest records `lines`, `bytes` and
`modules` precisely so that the failure says whether the shape changed or only
its contents. Recovering the rest means `--shape=<profile> --scale=<n> --record`
and reading the source by hand.

## Layout

```text
cli/benches/pinned/
  README.md
  mixed-100k.txt
  mixed-1M.txt
```

One file per corpus, `name = <basename>`, and nothing else in the directory that
ends in `.txt`. The fields are the ones a saved corpus's `manifest.txt` carries,
and they mean the same things — `params` is the full delta from
`Params::default()`, which is what the corpus is regenerated from.

## Running them

```text
cargo bench -p buri --bench compiler -- --set=scale     # the tier
cargo bench -p buri --bench compiler -- --set=scale --rss   # and peak memory
cargo bench -p buri --bench compiler -- --validate      # digests, without measuring
```

The tier is **not** in `--set=core` and not in `--set=full`: a million-line row
costs minutes and the default run has to stay something a contributor takes
before a commit. `--validate` covers the digests, `--validate --quick` does not
— that one is the CI gate and has to stay under a second, and regenerating a
million lines is not under a second.

## Pinning a new one

```text
cargo bench -p buri --bench compiler -- --pin=mixed-1M
BURI_BLESS=1 cargo bench -p buri --bench compiler -- --pin=mixed-1M   # re-pin
```

The profile and the scale come from the name, exactly as `--record`'s do
(`mixed-1M` is the `mixed` profile at 1,000,000 lines); `--shape`, `--scale`,
`--seed` and `--param` override any of it. It validates before it writes.

**A new scale point is a new manifest and nothing else.** The tier is every
`.txt` in this directory, so a 10M row would arrive as `mixed-10M.txt` and no
code change. It is deliberately absent: at the rates §6 records, a 10M native
row is minutes per repetition, and the question it would answer — is anything
superlinear — is one the 100k/1M pair already answers.

## Staleness

Same policy as `corpora/`, with one difference in the failure mode.

1. **A digest mismatch stops the run**, loudly, naming both digests and both
   sets of counts. It is not a warning: every number in the series the manifest
   anchors was taken over different bytes.
2. **The fix is a re-pin with a bumped `revision`**, in the same commit as the
   generator change that caused it. `--json` carries `corpus_revision`.
3. `GENERATOR_REVISION` is a note and never an error. A manifest pinned at an
   older revision whose digest still matches is the scheme working: the
   generator changed and these bytes did not.
4. A corpus that cannot be regenerated is deleted, not repaired.

## The digest

`corpus::digest` — `buri::build::cache::hash_bytes`, the same SHA-256 every
cache key uses — over the module path, a NUL, the module's bytes and a NUL, for
every module in sorted path order. Identical to what a saved corpus is checked
against, so the two kinds are comparable by construction: pinning a corpus that
is also checked in produces the same hash, which is how the generator's
byte-identity across a change is checked.
