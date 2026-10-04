---
title: A schema declares the edition this reader supports
message: '{declaration} is not accepted'
note: this reader implements edition {edition} and no other, and proto2 and proto3 differ from it in field presence, defaults on the wire and enum openness, so it would read any other file wrongly
fix: 'write `edition = "{edition}";`, and when migrating a `syntax` file, drop every `optional` and `required` label and write `[features.field_presence = IMPLICIT]` on the fields that had none'
reproduction: none
---
