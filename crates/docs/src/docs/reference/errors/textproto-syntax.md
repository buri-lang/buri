---
title: A text format file parses
message: '{problem}'
fix: '{remedy}'
reproduction: none
---
# A text format file parses

```textproto ignore why="a data file that does not parse"
name: "api"
ports: [80, 443]
```

```text
error: expected `,` or `]`, found `443` [textproto-syntax]
 --> lib/deploy/server.txtpb:2:12
```

The first thing that does not parse is reported, and nothing after it is
checked. A scalar takes `:`, a message takes `{ ... }` or `< ... >`, and a list
separates its values with `,`.
