## What it does

The static checks that are not type errors: sources declared but absent, a
source on disk that no rule names, a dependency that is declared and unused,
one that is used and undeclared, a visibility or tag violation, and a cycle in
the package graph.

Each finding carries a stable code, so a report can be grepped and a specific
check can be talked about by name.

These are separate from type checking because they are questions about the
*build graph* rather than about a program, and because a repository wants to be
able to run them without paying for a full compile.
