---
title: A message field's number and name are unique
message: '{problem}'
note: a number is what names a field on the wire, and a name what names it in JSON, so a second use would read as the first field
fix: '{remedy}'
reproduction: none
---
# A message field's number and name are unique

```proto
message Order {
  reserved 3, 10 to 19;
  reserved legacy_total;

  string id = 1;
  string note = 1;        // `note` and `id` both use field number 1
  int64 total = 12;       // `Order` reserves 12
  int64 legacy_total = 4; // `Order` reserves the name
}
```

A `oneof`'s cases count as fields of the message around them. `reserved`
keeps a deleted field's number and name from coming back with a new meaning,
so a field may use neither.
