# //lib/shop/lib.buri

A corner shop.

## Enums

### Payment

```buri sig
enum Payment
```

How a sale is paid.

- `Cash` — Notes and coins.
- `Card`
- `Tab` — A note to pay later.

## Structs

### Shelf

```buri sig
struct Shelf
```

What a shelf holds.

- `height: I64`
- `tins: I64` — How many tins fit.
- `label: Str`

## Traits

### Priced

```buri sig
trait Priced
```

Something with a price.

- `fn cents(self): I64`
- `fn units(self): I64` — The price in whole units, rounded down.

## Methods

*Pure* means the function takes no context parameter, so it cannot allocate, read, write, or observe anything — the guarantee is the absence of an argument rather than an annotation.

### Shelf.empty

```buri sig
fn empty(): Shelf
```

Pure.

### Shelf.stocked

```buri sig
fn stocked(self): Shelf
```

Pure.

A shelf one tin fuller.

## Functions

*Pure* means the function takes no context parameter, so it cannot allocate, read, write, or observe anything — the guarantee is the absence of an argument rather than an annotation.

### close

```buri sig
fn close(): Bool
```

Pure.

Closes the shop.

### first

```buri sig
fn first<T>(a: T, b: T): T
```

Pure.

The first of two prices.

### undocumented

```buri sig
fn undocumented(a: I64): I64
```

Pure.

