---
title: A library never imports its binary
message: '{path} belongs to the binary in {owner}'
label: a library may not reach the binary beside it
note: '{importer_file} belongs to the {rule} rule, and nothing depends on a binary'
fix: move what you need into the library, or into a third one both can depend on
reproduction: none
---
