---
title: An effect lives in an effect package
message: an effect is declared only in an effect package, under `//platform/effect/`
note: an effect package holds an effect, its wrapper functions and a `testing` surface, and every platform can share it
fix: 'move it into a library under `//platform/effect/`, such as `//platform/effect/kv`, or declare it as a plain `trait`'
---
# An effect lives in an effect package

```text
error: an effect is declared only in an effect package, under `//platform/effect/` [effect-outside-effect-directory]
```

```buri fail code=effect-outside-effect-directory
effect Mischief {
    fn meddle(self): ();
}
```

An effect is authority, so where one is declared is fixed. Effects live apart
from platforms, so two platforms can offer one effect and a library bounded by
it runs on both:

```text
platform/effect/kv/lib.buri           effect Kv, and get and put
platform/effect/kv/testing/lib.buri   TestKv
```
