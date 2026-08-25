---
title: An `export` reaches somebody
message: "`{name}` is exported and reaches nobody"
note: inside a library `export` means visible to the rest of the library, and `lib.buri` decides what leaves it
fix: re-export it from {library_file} to put it on the library's surface, or drop the `export`
---
