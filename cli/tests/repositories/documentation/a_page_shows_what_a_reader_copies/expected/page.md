# //lib/shown/lib.buri

Angles, shown the way a reader copies them.

## Functions

*Pure* means the function takes no context parameter, so it cannot allocate, read, write, or observe anything — the guarantee is the absence of an argument rather than an annotation.

### none

```buri sig
fn none(): Float
```

Pure.

No angle at all.

```buri
export fn flat(): Float {
    math.cos(0.0)
}
```

### right

```buri sig
fn right(): Float
```

Pure.

A right angle, in radians.

```buri
/*
# a note
*/
export fn level(): Float {
    math.sin(0.0)
}
```

