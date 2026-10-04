---
title: An entry with bodiless methods has a `js` file
message: '`{entry}` has no `js` file to implement `{name}`'
note: a method `platform.buri` declares without a body is the entry's `js` file's to implement
fix: name the file in the platform rule's `{entry}` entry, as in `js: "{entry}.mjs"`
reproduction: none
---
