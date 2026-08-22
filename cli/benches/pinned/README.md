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

## Layout, and what a name means

```text
cli/benches/pinned/
  README.md
  mixed-100k.txt
  mixed-1M.txt
  enum-heavy-100k.txt
  enum-heavy-1M.txt
  ...
```

One file per corpus, `name = <basename>`, and nothing else in the directory that
ends in `.txt`. The fields are the ones a saved corpus's `manifest.txt` carries,
and they mean the same things — `params` is the full delta from
`Params::default()`, which is what the corpus is regenerated from.

**A name is `<point>-<scale>`**, and the convention is load-bearing in three
places: `--pin=mixed-1M` reads the scale off the suffix, `--set=scale` reads the
anchor off the prefix, and the forty corpora here are twenty *points* at two
scales rather than forty unrelated corpora. The point — not the profile — is
what identifies a corpus across scales; four of the twenty are the `mixed`
profile with a parameter delta, and their manifests say exactly that.

One optional field the saved kind does not use:

```text
native = false
```

Absent means `true`. A native lowering row costs about thirty times a JavaScript
one, so it is spent where the backend is the question and not everywhere; §4 of
`design/PERFORMANCE.md` lists which seven points carry it and why. `--pin` reads
it off `--targets`: a pin taken with `--targets=js` records `native = false`.

## The twenty points

Every point has its own seed, and its two scales share it — so a point's 1M
corpus contains its 100k corpus's modules and then some, and the only thing that
differs between the two rows is the size. Sixteen of the twenty are named
profiles (`--list`); four are the `mixed` profile with one weight moved, because
they were worth a scale row and not yet worth a profile.

| Point | Profile / delta | The axis it moves |
|---|---|---|
| `mixed` | `mixed` | The anchor. Every other point is a delta from it. |
| `mixed-many-files` | `mixed-many-files` | Module count: 1,568 at 100k, 15,640 at 1M. |
| `mixed-few-files` | `mixed-few-files` | Module size: 21 modules at 100k, 192 at 1M. |
| `mixed-libs` | `mixed-libs` | A clustered import graph with thin edges between clusters. |
| `mixed-deep-graph` | `mixed-deep-graph` | A deep dependency chain rather than a wide fan. |
| `mixed-wide-graph` | `mixed-wide-graph` | Import fan-out at a fixed line count. |
| `struct-heavy` | `struct-heavy` | Layout, field resolution, wide records. |
| `struct-light` | `struct-light` | The control for the row above. |
| `enum-heavy` | `enum-heavy` | Wide enums matched exhaustively — the fewest functions per line of any point. |
| `generic-blowup` | `generic-blowup` | Monomorphization: 243k functions at 1M against the anchor's 132k. |
| `derive-heavy` | `derive-heavy` | `middle::derives`, which only the native branch runs. |
| `impl-heavy` | `impl-heavy` | Method resolution and per-impl setup. |
| `match-heavy` | `match-heavy` | Decision-tree construction at realistic arm counts. |
| `comment-heavy` | `comment-heavy` | 46 bytes a line: the lexer's comment path. |
| `comment-free` | `comment-free` | 29 bytes a line. The *ratio* of the two is the number. |
| `long-idents` | `long-idents` | Bytes per token at a fixed token count. |
| `string-heavy` | `mixed` + `w_string_fn=8` | String literals and interpolation. |
| `list-heavy` | `mixed` + `w_list_fn=8` | List literals, closures, the generic list operations. |
| `long-bodies` | `mixed` + `w_arith_fn=8 body_lets=24..48 nesting=3` | Per-body cost inside a realistic mix. |
| `generic-free` | `mixed` + `w_generic_fn=0 generic_args=1` | The control for `generic-blowup`. |

## Running them

```text
cargo bench -p buri --bench compiler -- --set=scale        # the sample: ~9 min
cargo bench -p buri --bench compiler -- --set=scale-full   # all forty: ~25 min
cargo bench -p buri --bench compiler -- --set=scale --rss  # and peak memory
cargo bench -p buri --bench compiler -- --only=enum-heavy --set=scale-full
```

Neither set is in `core` and neither is in `full`: a million-line row costs
minutes and the default run has to stay something a contributor takes before a
commit.

**`--set=scale` is the sample and `--set=scale-full` is the sweep.** The sample
is every pinned corpus the standard protocol applies to — the whole 100k tier —
plus `mixed-1M`, the one 1M corpus every other 1M corpus is a delta from. The
threshold is the same 500,000 lines the repetition deviation already uses, so
there is one number rather than two. `--only=` cuts either of them to a point or
a scale, which is how a suspected outlier is re-run.

The same split governs `--validate`, because regenerating and digesting forty
corpora is minutes:

| Command | Pinned corpora covered | Wall time |
|---|---|---|
| `--validate --quick` | none — the CI gate, and it has to stay under a second | 0.3 s |
| `--validate` | the anchor, `mixed-100k` and `mixed-1M` | 10 s |
| `--validate --set=scale` | the sample: the 100k tier and `mixed-1M` | 21 s |
| `--validate --set=scale-full` | all forty | 2 min 47 s |

## Pinning a new one

```text
cargo bench -p buri --bench compiler -- --pin=mixed-1M
cargo bench -p buri --bench compiler -- --pin=string-heavy-1M --shape=mixed \
    --seed=0x0B001A575EED0011 --targets=js --param w_string_fn=8
BURI_BLESS=1 cargo bench -p buri --bench compiler -- --pin=mixed-1M   # re-pin
```

The profile and the scale come from the name, exactly as `--record`'s do
(`mixed-1M` is the `mixed` profile at 1,000,000 lines); `--shape`, `--scale`,
`--seed`, `--targets` and `--param` override any of it. A point that is not a
profile needs `--shape` to say which profile it is a delta from. It validates
before it writes.

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
   generator change that caused it. `--json` carries `corpus_revision`. Forty
   re-pins is a script over `--list`, and it is still forty deliberate acts.
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
