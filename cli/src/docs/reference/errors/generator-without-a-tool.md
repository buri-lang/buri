---
title: A generator names the program that runs it
message: this `generators` entry names no tool
note: a generator is its tool, so an entry without one names nothing the build could run
fix: 'add `tool:` — a `//label` naming a `tool` rule, or `std/proto`'
reproduction: none
---
