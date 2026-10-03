---
title: An output names its entries and its variant
message: '`{name}` is retired'
fix: '{replacement}'
reproduction: none
---
# An output names its entries and its variant

| Was                  | Now                                            |
| -------------------- | ---------------------------------------------- |
| `arch: ARM64`        | `variant: "linux-arm64"`                       |
| `entry: "run"`       | `entries: [{ name: "main", function: "run" }]` |
| `js { module: ESM }` | nothing: every JavaScript output is an ES module |
