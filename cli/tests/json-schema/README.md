# The JSON Schema test suite

The official [JSON-Schema-Test-Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite), draft 2020-12, run against the in-tree validator:

```sh
cargo test -p buri --lib json_schema_test_suite -- --nocapture
```

The harness is `cli/src/languages/json/suite.rs`. It prints a tally per file, and holds the list of groups left out, each with the finding it must draw instead.

## What was vendored, and from where

From upstream commit `5b0ee1613e45fcc2bddac00e07c19cd49b00d8a8` (2026-09-21), verbatim:

| Vendored | Origin |
|---|---|
| `tests/*.json` | `tests/draft2020-12/*.json`, all of them |
| `remotes/draft2020-12/` | the remotes those tests reach through `http://localhost:1234/` |
| `LICENSE` | `LICENSE`. The suite is MIT. |

`optional/` isn't vendored. Neither are the remotes only it or `vocabulary.json` use: `format-assertion-*.json`, `metaschema-*.json` and `prefixItems.json`.
