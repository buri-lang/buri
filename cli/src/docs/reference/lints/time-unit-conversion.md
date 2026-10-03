---
title: A length of time is a `Duration`
severity: warning
message: this spells the {units} conversion out in integers
note: "`core/time`'s `Duration` is the length itself — `time.milliseconds(n)`, `time.seconds(n)` — and its arithmetic saturates, so a deadline built from one cannot overflow into the past"
fix: build a `Duration` and let the unit live in the type rather than in the name
---
An `I64` of milliseconds carries its unit only in its name, like
`IDLE_TIMEOUT_MILLIS`, and every conversion is a hand-written multiply that can
drop, double or overflow the unit unnoticed. Use `core/time` instead:

```
let idle = time.minutes(5);
let deadline = started.plus(idle);
if (now.hasPassed(deadline)) { … }
```

- Constructors: `seconds`, `milliseconds`, `microseconds`, `nanoseconds`,
  `minutes`, `hours`.
- Arithmetic: `add`, `subtract`, `multiply`, `negate`, `abs`.
- Readers: `nanoseconds()`, `milliseconds()` and the rest, in any unit.

You never need a conversion factor. Two things come with the type:

- **The arithmetic saturates.** An oversized length becomes the largest length,
  not a negative one, so a deadline can't overflow into the past.
- **The unit is in the type.** `Duration` and `Instant` differ on purpose: a
  length plus a point is a point, two points make a length, and adding two
  points is a type error.

The rule fires on constants named as conversions, like
`NANOSECONDS_PER_MILLISECOND` or `MILLISECONDS_PER_SECOND`, and on a millisecond
count multiplied by a million in place. A million not named as milliseconds is
left alone, since parts per million is a real thing.
