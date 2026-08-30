#!/usr/bin/env bash
# **Does the message say what to fix, and where?**
#
# Every other assertion about a diagnostic in this repository is about its
# code, its span, its `fix` field being present. None of them can say whether
# the sentence is any good, because that is a judgement about English. This
# puts each one to a model with a one-question rubric and records the ones it
# says no to.
#
# Out of `cargo test` on purpose, the way `cli/tests/proto/run.sh` is: it needs
# the network and a model, and a suite that cannot run without either is a
# suite that does not run. It is also gated on an environment variable, so that
# nothing reaches the network by accident.
#
#   BURI_MESSAGE_AUDIT=1 ./run.sh                 audit the whole corpus
#   BURI_MESSAGE_AUDIT=1 ./run.sh --sampled 5     five generated cases, not sixty
#   BURI_MESSAGE_AUDIT=1 ./run.sh --out r.md      where the report goes
#   BURI_MESSAGE_AUDIT=1 ./run.sh --batch 20      records per request
#   BURI_MESSAGE_AUDIT=1 ./run.sh --limit 5       grade only the first five
#
# `codex` has to be on PATH (codex-cli 0.151.0 or newer). The corpus comes from
# `message_audit_corpus` in `cli/tests/recovery.rs`: the curated cases first,
# then a deterministic spread of the generated ones.
#
# The verdicts are advice, not a gate. Nothing here fails a build — it writes a
# list of messages a reader thought were unclear, which is the input to the next
# round of wording rather than the judge of the last one.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"

sampled=60
batch=12
limit=0
out="${TMPDIR:-/tmp}/buri-message-audit.md"
model="${BURI_MESSAGE_AUDIT_MODEL:-gpt-5.6-luna}"

while [ $# -gt 0 ]; do
  case "$1" in
    --sampled) sampled="$2"; shift 2 ;;
    --batch) batch="$2"; shift 2 ;;
    --limit) limit="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --model) model="$2"; shift 2 ;;
    *) echo "error: unknown argument $1" >&2; exit 2 ;;
  esac
done

if [ "${BURI_MESSAGE_AUDIT:-}" != "1" ]; then
  cat >&2 <<'MSG'
error: this reaches the network, so it runs only when asked.

  BURI_MESSAGE_AUDIT=1 ./run.sh

MSG
  exit 2
fi

if ! command -v codex >/dev/null 2>&1; then
  echo "error: codex is not on PATH. See https://github.com/openai/codex." >&2
  exit 2
fi

cases="$(mktemp "${TMPDIR:-/tmp}/buri-audit-cases.XXXXXX")"
replies="$(mktemp "${TMPDIR:-/tmp}/buri-audit-replies.XXXXXX")"
trap 'rm -rf "$cases" "$replies" "$cases.parts"' EXIT

echo "collecting the corpus…" >&2
(
  cd "$root"
  BURI_MESSAGE_AUDIT=1 \
  BURI_MESSAGE_AUDIT_CASES="$cases" \
  BURI_MESSAGE_AUDIT_SAMPLED="$sampled" \
    cargo test -p buri --test recovery -- --ignored --nocapture message_audit_corpus \
    >/dev/null
)

if [ "$limit" -gt 0 ]; then
  awk -v limit="$limit" '/^--- / { n++ } n <= limit' "$cases" > "$cases.cut"
  mv "$cases.cut" "$cases"
fi

total="$(grep -c '^--- ' "$cases" || true)"
if [ "${total:-0}" -eq 0 ]; then
  echo "error: the corpus is empty. Did cli/tests/recovery.rs build?" >&2
  exit 1
fi
echo "$total records, $batch to a request, model $model" >&2

# The rubric. One question, one line of answer, and nothing that invites prose:
# a grader that writes paragraphs cannot be counted.
read -r -d '' rubric <<'RUBRIC' || true
You are auditing compiler error messages for the Buri language. For each
numbered record you are given the source line, the message, and the `fix` the
message offers.

One question per record: **does the message name exactly what the reader must
fix, and where?** Answer NO if it names a token the reader did not write and
would not think of (a closing brace, when what is missing is a comma), if it
describes the parser's confusion rather than the reader's mistake, if the
location is not where the mistake is, or if the fix does not say what to type.
Answer YES only if a reader who had never seen the parser could act on it
immediately.

Answer with one line per record and nothing else, in this exact form:

<number>: YES — <one clause>
<number>: NO — <one clause>

Records:
RUBRIC

# awk splits the records into batches rather than a loop over `sed`, so a
# corpus of any size is one pass.
mkdir -p "$cases.parts"
rm -f "$cases.parts"/*
awk -v batch="$batch" -v dir="$cases.parts" '
  /^--- / { n++; part = int((n - 1) / batch) }
  { print > sprintf("%s/%03d", dir, part) }
' "$cases"

: > "$replies"
for part in "$cases.parts"/*; do
  echo "  $(basename "$part") …" >&2
  prompt="$rubric
$(cat "$part")"
  # `--sandbox read-only` because the grader reads and answers; it has no
  # business writing anything, and the one thing it must not do is edit the
  # messages it is grading.
  ( cd "$root" && codex exec --sandbox read-only -m "$model" "$prompt" </dev/null 2>/dev/null ) \
    | grep -E '^[0-9]+: (YES|NO)' >> "$replies" || true
done

# `codex exec` echoes its final message after the transcript, so every verdict
# arrives twice. The first one wins.
sorted="$(mktemp "${TMPDIR:-/tmp}/buri-audit-sorted.XXXXXX")"
awk -F: '!seen[$1]++' "$replies" | sort -t: -k1,1n > "$sorted"

yes="$(grep -c ': YES' "$sorted" || true)"
no="$(grep -c ': NO' "$sorted" || true)"

{
  echo "# Message audit"
  echo
  echo "$total records, $((yes + no)) graded by \`$model\`: $yes yes, $no no."
  echo
  echo "Every NO, with the record it was about:"
  echo
  while IFS= read -r verdict; do
    case "$verdict" in
      *": NO"*) ;;
      *) continue ;;
    esac
    id="${verdict%%:*}"
    echo '```'
    awk -v id="$id" '
      $0 == "--- " id { on = 1; next }
      /^--- / { on = 0 }
      on && NF { print }
    ' "$cases"
    echo "verdict: ${verdict#*: }"
    echo '```'
    echo
  done < "$sorted"
} > "$out"

rm -f "$sorted"
echo "wrote $out — $no of $((yes + no)) messages were marked NO" >&2
