---
title: A value crosses to a `js` file by the crossing table
message: '`{type}` cannot cross to JavaScript, as {place}'
note: 'the JS backend crosses `Str`, `Bool`, `F64`, integers, `[U8]`, lists, tuples, `Option`, `()`, structs, `Request` and `Response`, and `Result<T, Str>` as a whole answer'
fix: take or answer a type from the crossing table, such as a struct of them
reproduction: none
---
# A value crosses to a `js` file by the crossing table

```text
error: `Char` cannot cross to JavaScript, as `HostKv.get`'s parameter `key` [type-cannot-cross]
```

An entry's `js` file and the methods it implements meet Buri at a boundary, and
only these types cross it:

| Buri                    | JavaScript                     |
|---|---|
| `Str`, `Bool`, `F64`    | `string`, `boolean`, `number`  |
| `Int`, other integers   | `bigint`                       |
| `[U8]`                  | `Uint8Array`                   |
| `[T]`, a tuple          | `Array`                        |
| `Option<T>`, `()`       | the value, or `undefined`      |
| `Result<T, Str>`        | the value, or a thrown `Error` |
| a struct                | a plain object of its fields   |
| `Request`, `Response`   | the Fetch standard's           |

`Result` crosses only as a whole answer. `Option<Option<T>>` doesn't cross,
because `Some(None)` would be `undefined` too.
