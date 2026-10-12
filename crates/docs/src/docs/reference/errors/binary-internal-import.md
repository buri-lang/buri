---
title: A binary imports its library through the surface
message: '{path} is internal to the library {owner}'
label: the binary reaches the library only through its entry point
note: only names re-exported by {library_file} are available, and {importer_file} belongs to the {rule} rule rather than the library's
fix: 'import the library instead: from "{surface}" import {{ ... }}'
reproduction: none
---
