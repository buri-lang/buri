---
title: Every delimiter a construct opens is closed
message: this {construct} is missing its closing {token}
fix: write {token} here
---
# Every delimiter a construct opens is closed

```text
error: this `match` is missing its closing `}` [unclosed-delimiter]
```

```buri fail code=unclosed-delimiter
fn seeds(): [Int] {
  [1, 2, 3
}
```

An unclosed construct claims the next closer it meets, and every error after
that would be about the miscount. Naming the opener stops that cascade.

Where an unclosed block ends is the parser's guess, so this is the only error
you get about it. The editor still completes and hovers the names bound inside,
but nothing else in it is reported until you close it.
