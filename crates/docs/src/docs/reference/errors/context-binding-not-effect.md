---
title: A context binding names an effect
message: '{binding} is not an effect'
note: a context binds effects; a trait or any other type is not one
fix: bind an effect, as in `Allocator: host.alloc`; pass a plain trait's implementations as ordinary arguments
reproduction: none
---
