---
title: A generator names the program that runs it
message: this `generators` entry names no tool
note: a generator is its tool, so an entry without one names nothing the build could run
fix: 'add `tool:` — a `//label` naming a binary in this repository, or `std/codegen/proto`'
reproduction: none
---
