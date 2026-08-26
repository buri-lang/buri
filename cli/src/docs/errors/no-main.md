---
title: A binary is entered through `main`
message: '{package} exports no `main`'
fix: add `export fn main(): Result<(), Str> {{ ... }}` to its `main.buri`
reproduction: none
---
