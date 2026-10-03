---
title: A single-value field is set once
message: '`{field}` is set twice, and it holds one value'
fix: delete one of the two, or make the field `repeated` in the schema
reproduction: none
---
# A single-value field is set once

```textproto ignore why="a data file, not a build file"
name: "api"
name: "web"

# `name` is set twice
region: "eu"
zone: "eu-west-1a"
# `region` and `zone` are cases of one `oneof`
```

A `repeated` field may be written any number of times, and its values join in
order. Any other field holds one value, and so does a `oneof`, whichever case
holds it.
