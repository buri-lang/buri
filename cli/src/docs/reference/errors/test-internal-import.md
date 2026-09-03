---
title: A test reaches its library the way a dependent does
message: '{test_source} imports a library-internal module'
label: internal to the library under test
note: tests reach their library the same way dependents do
fix: 'import {owner}, and re-export {exports} from {owner_path}/lib.buri if it is part of the surface you meant to test'
reproduction: none
---
