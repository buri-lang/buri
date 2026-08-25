---
title: A `testing/` directory is declared by a `testing` block
message: {package} has a testing/ directory and no `testing` block
note: the block is what puts the surface in the build; without it nothing compiles testing/lib.buri and no dependent can name it
fix: add a `testing {{ }}` block to the library rule in {package_path}/BUILD.buri — empty if the entry point is the whole of it — or delete the directory
reproduction: none
---
