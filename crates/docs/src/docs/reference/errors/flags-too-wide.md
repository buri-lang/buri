---
title: A `Flags` type has at most 64 fields
message: '`{type}` cannot derive `Flags`: it has {count} fields, and the most is {max}'
note: a `Flags` type is stored in one unsigned word, and the widest is a `U64`
fix: split the flags into two `Flags` types, and hold both
---
# A `Flags` type has at most 64 fields

```text
error: `Wide` cannot derive `Flags`: it has 65 fields, and the most is 64 [flags-too-wide]
```

```buri fail code=flags-too-wide
# from "core/flags" import { Flags };

derive Flags for Wide;
struct Wide {
    f0: Bool,
#     f1: Bool,
#     f2: Bool,
#     f3: Bool,
#     f4: Bool,
#     f5: Bool,
#     f6: Bool,
#     f7: Bool,
#     f8: Bool,
#     f9: Bool,
#     f10: Bool,
#     f11: Bool,
#     f12: Bool,
#     f13: Bool,
#     f14: Bool,
#     f15: Bool,
#     f16: Bool,
#     f17: Bool,
#     f18: Bool,
#     f19: Bool,
#     f20: Bool,
#     f21: Bool,
#     f22: Bool,
#     f23: Bool,
#     f24: Bool,
#     f25: Bool,
#     f26: Bool,
#     f27: Bool,
#     f28: Bool,
#     f29: Bool,
#     f30: Bool,
#     f31: Bool,
#     f32: Bool,
#     f33: Bool,
#     f34: Bool,
#     f35: Bool,
#     f36: Bool,
#     f37: Bool,
#     f38: Bool,
#     f39: Bool,
#     f40: Bool,
#     f41: Bool,
#     f42: Bool,
#     f43: Bool,
#     f44: Bool,
#     f45: Bool,
#     f46: Bool,
#     f47: Bool,
#     f48: Bool,
#     f49: Bool,
#     f50: Bool,
#     f51: Bool,
#     f52: Bool,
#     f53: Bool,
#     f54: Bool,
#     f55: Bool,
#     f56: Bool,
#     f57: Bool,
#     f58: Bool,
#     f59: Bool,
#     f60: Bool,
#     f61: Bool,
#     f62: Bool,
#     f63: Bool,
    f64: Bool,
}
```
