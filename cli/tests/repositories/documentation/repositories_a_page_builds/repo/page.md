# Repositories a page builds

A build file that breaks its schema.

```textproto schema=build
library {
    bogus: 1
}
```

A `REPO.buri` that keeps to its own.

```textproto schema=repo
# Nothing to configure.
```

The page's own repository, built from an empty `REPO.buri`.

```textproto file=lib/a/BUILD.buri
library {}
```

```buri file=lib/a/lib.buri
export fn a(): I64 {
    1
}
```

```textproto file=lib/a/BUILD.buri
library {}
```

A fence showing a mistake that isn't one.

```buri file=lib/b/lib.buri
export fn b(): I64 {
    1
}
```

```textproto fail code=build-unknown-field file=lib/b/BUILD.buri
library {}
```

A fence showing a mistake that fails with another code.

```textproto fail code=build-unknown-field file=lib/c/BUILD.buri
library {
    dependencies: ["//lib/nowhere"]
}
```

A file of a repository that isn't there.

```textproto file=lib/d/BUILD.buri repo=missing
library {}
```

A file of `sample` that doesn't type check, and a mistake in its build file.

```buri file=lib/money/lib.buri repo=sample
export fn latte(): I64 {
    "450"
}
```

```textproto fail code=build-unknown-field file=lib/money/BUILD.buri repo=sample
library {
    bogus: 1
}
```

A mistake in a repository that isn't there.

```textproto fail code=build-unknown-field file=lib/e/BUILD.buri repo=absent
library {
    bogus: 1
}
```
