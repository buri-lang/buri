## Errors are not ignorable

`Result` is must-use: `let _ = fs.writeText(ctx, p, body);` does not compile.
Since Buri has no expression statements, `let _ =` is the only way to discard a
value, so the rule has no holes. Consume a `Result` with `?`, with `match`, with
`result.withDefault`, or — when you really mean it — with the explicit,
greppable `result.ignore`.
