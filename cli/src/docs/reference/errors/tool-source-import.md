---
title: Nothing imports a tool's modules
message: '{path} belongs to the tool in {owner}'
label: only the tool and its tests reach its modules
note: '{importer_file} belongs to the {rule} rule, and the toolchain is what calls a tool'
fix: move what you need into a library, and depend on it from both
reproduction: none
---
# Nothing imports a tool's modules

A tool is run by the build, not linked into a program. Code a tool shares with a
library or a binary lives in a library that all of them list in `dependencies`.
