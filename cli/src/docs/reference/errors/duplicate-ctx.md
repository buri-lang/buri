---
title: A function takes one context
message: a function has at most one `ctx` parameter
note: a function cannot take two independent contexts; bundle them into one type instead
fix: combine the two into one context, which is what `context {{ ..base, ... }}` is for
reproduction: none
---
