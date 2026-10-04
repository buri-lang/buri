---
title: A binary imports its library through the surface
message: '{path} is internal to the library {owner}'
label: the binary reaches the library only through its entry point
note: only names re-exported by {owner_path}/lib.buri are available, and {importer_file} belongs to the {rule} rule rather than the library's
fix: 'import the library instead: from "{owner}" import {{ ... }}'
reproduction: none
---
