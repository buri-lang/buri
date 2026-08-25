---
title: A schema declares its edition
message: the file does not declare its edition
note: a file with no declaration is proto2 to every other tool, and proto2 is not what this mapping implements
fix: add `edition = "{edition}";` as the first line
reproduction: none
---
