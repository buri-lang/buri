## 2. Notation and conformance

The normative grammar lives in [`grammar.ebnf`](./cli/src/docs/grammar.ebnf). Where this
document and that file disagree about syntax, the grammar file wins. Where this
document states a rule that the grammar cannot express, that rule is normative
and is checked after parsing; `design/static-rules.md` indexes every one of
them.

Terminology: *must* is a requirement on conforming programs and implementations;
*should* is a recommendation; *may* grants latitude.

---
