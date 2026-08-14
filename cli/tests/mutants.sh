#!/usr/bin/env bash
# Mutation testing for the Buri toolchain.
#
# A test suite that passes proves nothing on its own — it has to be capable of
# failing. This injects a specific, realistic bug into the compiler or its
# runtime, runs the suite, and reports whether the bug was caught. A mutant
# that SURVIVES is a hole in the tests.
#
#   cli/tests/mutants.sh            run every mutant
#   cli/tests/mutants.sh divide     run the ones whose name matches
#
# Each mutant is a `name|file|search|replace` record. The search text must
# appear exactly once, which is checked, so a mutant cannot silently no-op.

set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
ROOT=$PWD
FILTER=${1:-}

MUTANTS=(
  "divide-truncation|cli/src/runtime.js|  if (b === 0) \$divz();
  return Math.trunc(a / b);|  if (b === 0) \$divz();
  return a / b;"

  "remainder-sign|cli/src/runtime.js|function \$remi(a, b) {
  if (b === 0) \$divz();
  return a % b;
}|function \$remi(a, b) {
  if (b === 0) \$divz();
  return Math.abs(a % b);
}"

  "overflow-not-checked|cli/src/runtime.js|  if (v < lo || v > hi) \$crash(\"integer overflow\");
  return v;|  return v;"

  "divide-by-zero-allowed|cli/src/runtime.js|function \$divb(a, b) {
  if (b === 0n) \$divz();
  return a / b;
}|function \$divb(a, b) {
  if (b === 0n) return 0n;
  return a / b;
}"

  "comparison-order|cli/src/runtime.js|  // out of the comparisons below.
  return a < b ? 0 : a > b ? 2 : 1;|  // out of the comparisons below.
  return a < b ? 2 : a > b ? 0 : 1;"

  "structural-equality|cli/src/runtime.js|    for (let i = 0; i < a.length; i++) if (!\$eq(a[i], b[i])) return false;|    for (let i = 1; i < a.length; i++) if (!\$eq(a[i], b[i])) return false;"

  "sort-not-stable|cli/src/runtime.js|      return o === 1 ? a[1] - b[1] : o === 0 ? -1 : 1;|      return o === 1 ? b[1] - a[1] : o === 0 ? -1 : 1;"

  "string-length-utf16|cli/src/runtime.js|function \$str_len(s) {
  return BigInt(\$chars(s).length);
}|function \$str_len(s) {
  return BigInt(s.length);
}"

  "index-bounds|cli/src/runtime.js|  return n >= 0 && n < xs.length ? \$some(xs[n]) : \$none;|  return n < xs.length ? \$some(xs[n]) : \$none;"

  "wrapping-conversion|cli/src/runtime.js|function \$convChecked(v, lo, hi, target, toBig) {|function \$convChecked(v, lo, hi, target, toBig) {
  if (true) return \$ok(toBig ? BigInt(Math.trunc(Number(v))) : Number(v));"

  "tail-calls-off|cli/src/tco.rs|        if scc.len() == 1 {|        if true {"

  "exhaustiveness-off|cli/src/exhaust.rs|    if let Some(witness) = ctx.useful(&covering, &[Pat::Wild], &types) {|    if false {
        let witness: Vec<Witness> = Vec::new();"

  "must-use-off|cli/src/infer_expr.rs|        if matches!(pattern, ast::Pattern::Wild { .. }) && self.is_result(&ty) {|        if false {"

  "short-circuit-off|cli/src/codegen.rs|                let cond = if is_and {
                    Expr::ident(name.clone())
                } else {
                    Expr::un(UnOp::Not, Expr::ident(name.clone()))
                };|                let cond = Expr::Bool(true);"

  "coalesce-not-lazy|cli/src/codegen.rs|                if r_stmts.is_empty() {
                    return Expr::cond(
                        ok,
                        Expr::index(Expr::ident(name.clone()), Expr::Num(1.0)),
                        r,
                    );
                }|                if r_stmts.is_empty() {
                    return Expr::Seq(vec![
                        r.clone(),
                        Expr::cond(
                            ok,
                            Expr::index(Expr::ident(name.clone()), Expr::Num(1.0)),
                            r,
                        ),
                    ]);
                }"

  "context-spread-dropped|cli/src/infer_expr.rs|                    for (t, impl_ty) in base_bindings {|                    for (t, impl_ty) in base_bindings.into_iter().take(0) {"

  "or-pattern-hoist|cli/src/codegen.rs|        self.hoist_or_declarations(&arm.pattern, out);
        let mut body = Vec::new();|        let mut body = Vec::new();"

  "literal-range-off|cli/src/infer.rs|            if !fits {|            if false {"
)

backup() { cp "$1" "/tmp/mutant-backup-$(echo "$1" | tr / _)"; }
restore() { cp "/tmp/mutant-backup-$(echo "$1" | tr / _)" "$1"; }

caught=0
survived=0
skipped=0
survivors=()

for record in "${MUTANTS[@]}"; do
  name=${record%%|*}
  rest=${record#*|}
  file=${rest%%|*}
  rest=${rest#*|}
  search=${rest%%|*}
  replace=${rest#*|}

  if [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]]; then continue; fi

  backup "$file"
  applied=$(SEARCH="$search" REPLACE="$replace" python3 - "$file" <<'PY'
import os, sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
search, replace = os.environ["SEARCH"], os.environ["REPLACE"]
n = s.count(search)
if n != 1:
    print(f"SKIP:{n}")
else:
    p.write_text(s.replace(search, replace))
    print("OK")
PY
)
  if [[ "$applied" == SKIP:* ]]; then
    printf '  ??  %-24s search text appears %s times, not once\n' "$name" "${applied#SKIP:}"
    skipped=$((skipped + 1))
    restore "$file"
    continue
  fi

  if ! cargo build -p buri --release >/dev/null 2>&1; then
    # A mutant that does not compile is not a useful mutant, but it is also
    # not a hole in the tests.
    printf '  --  %-24s does not compile\n' "$name"
    skipped=$((skipped + 1))
    restore "$file"
    continue
  fi

  if cargo test -p buri --release --test conformance >/dev/null 2>&1; then
    printf '  !!  %-24s SURVIVED — no test catches this\n' "$name"
    survived=$((survived + 1))
    survivors+=("$name")
  else
    printf '  ok  %-24s caught\n' "$name"
    caught=$((caught + 1))
  fi
  restore "$file"
done

cargo build -p buri --release >/dev/null 2>&1
echo
echo "$caught caught, $survived survived, $skipped skipped"
if [ "$survived" -gt 0 ]; then
  echo "survivors: ${survivors[*]}"
  exit 1
fi
