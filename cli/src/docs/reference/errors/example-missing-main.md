---
title: A runnable example exports `main`
message: this example exports no `main`
fix: give the fence `wrap=body`, which supplies one, or write `export fn main(host: NodeHost): Result<(), Str> {{ ... }}`
reproduction: none
---
