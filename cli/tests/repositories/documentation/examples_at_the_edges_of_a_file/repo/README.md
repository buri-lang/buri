# Build files

A blank line between a rule's fields stays when the rule is laid out.

```textproto schema=build
binary {
    outputs: [{ platform: "node" }]

    test { sources: ["t.buri"] }
}
```
