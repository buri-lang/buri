---
title: An output's `entries` name functions
message: '`{entry}` is not a function name'
note: each `function` in `entries` names an exported function of the binary's `main.buri`, so it is a Buri identifier — letters, digits and `_`, not starting with a digit
fix: write the function's name, or drop the item from `entries` to fill the entry with the function of its own name
reproduction: none
---
