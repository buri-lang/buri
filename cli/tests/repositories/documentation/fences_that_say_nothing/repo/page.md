# Fences that say nothing

An info string that doesn't parse.

```buri file="lib/a b
```

```buri =lonely
```

```buri wrap=body wrap=expr
```

```buri run fail
```

Textproto fences that don't say what they are.

```textproto schema=bogus
```

```textproto repo=elsewhere
```

```textproto code=some-code
```

```textproto
name: "nothing"
```

```textproto fail file=BUILD.buri
```

```textproto fail code=some-code file=BUILD.buri schema=build
```

```textproto check
```

Buri fences that claim what they can't.

```buri wrap=bogus
```

```buri role=bogus
```

```buri wrap=body ctx="alloc, bogus"
```

```buri
export fn one(): Int {
    1
}
```

```stdout
1
```

```buri run
let a = 1; // ERROR: nothing
```

```buri run
export fn main() {}
```

```buri fail
export fn one(): Int {
    1
}
```

```buri lint
export fn one(): Int {
    1
}
```

```buri explode
```

```buri code=some-code
```

```buri sig file=lib/a/lib.buri
```

```buri wrap=body file=lib/a/lib.buri
```

```buri platform=moon
```

```buri file=lib/a/lib.buri
export fn a(): I64 {
    let b: I64 = "b"; // ERROR: expected
    1
}
```

A plain fence, and one in another language, are prose.

```
anything
```

```text
anything
```

The last fence never closes, and is compiled to the end of the page.

```buri
export fn two(): Int {
    2
}
