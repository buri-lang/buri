---
title: A schema declares an edition, not a syntax
message: '{declaration} is not accepted'
note: this reader implements Protobuf Editions. proto2 and proto3 differ from it in field presence, in what a default means on the wire, and in whether an enum is open — so a `syntax` file is not a file it can read a little differently, it is a file it would read wrongly
fix: 'migrate it: `edition = "{edition}";`, drop every `optional` and `required` label, and write `[features.field_presence = IMPLICIT]` on the fields that had none'
reproduction: none
---
