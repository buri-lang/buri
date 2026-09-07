---
title: A generated module has a name nothing else uses
message: '`{module}` is a module of this rule already'
fix: give the generated module a name the package does not already declare
reproduction: none
---
# A generated module has a name nothing else uses

A name belongs to a generator or to a person, never to both. Two `generators`
entries that name one module, or a generated module named after a file in the
package, would leave two declarations of the same module path — and only one of
them can be the one the build compiles.

Whichever was there first wins, so the file on disk keeps meaning what it says.
The note says which of the two it was: another entry on the rule, or a source of
the package.

`lib.buri` is the one worth naming on its own. It is a library's whole public
surface, so a generator that took it over would decide what leaves the library,
and nothing in the package would say so.

A schema is not a collision. `std/codegen/proto` names its module `point.proto`
and `lib/wire/point.proto` is a file on disk; that file is the generator's
input, not a module anybody imports.
