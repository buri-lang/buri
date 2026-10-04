---
title: A derived operator needs a single-field struct
message: '`{type}` derives `{operator}` only for a single-field struct'
note: an arithmetic newtype wraps exactly one value
fix: write the `impl` by hand, or wrap exactly one value
reproduction: none
---
