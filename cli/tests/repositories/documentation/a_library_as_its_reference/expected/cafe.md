# //lib/cafe/lib.buri

The café's counter.

Orders, the menu, and the till.

## Structs

### Card

```buri sig
struct Card
```

A loyalty card.

- `stamps: I64`

### Drink

```buri sig
struct Drink
```

A drink on the menu.

- `name: Str` — What it's called at the counter.
- `cents: I64`

## Enums

### Serving

```buri sig
enum Serving
```

How a drink is served.

- `Here`
- `ToGo`

## Type aliases

### Cents

```buri sig
type Cents = I64
```

An amount of money, in cents.

## Traits

### Priced

```buri sig
trait Priced
```

Something the till can ring up.

- `fn cents(self): I64` — What it costs, in cents.

## Constants

### STAMPS

```buri sig
let STAMPS: I64
```

How many drinks a card holds before one is free.

## Contexts

### Shift

```buri sig
context Shift
```

What the till runs with.

## Methods

*Pure* means the function takes no context parameter, so it cannot allocate, read, write, or observe anything — the guarantee is the absence of an argument rather than an annotation.

### Card.cents — via Priced

```buri sig
fn cents(self): I64
```

Pure.

### Card.equal — via Equal

```buri sig
fn equal(self, other: Self): Bool
```

Pure.

### Card.show — via Show

```buri sig
fn show<C: Allocator>(self, ctx: C): Str
```

Effects: `Allocator`

### Card.stamped

```buri sig
fn stamped(self): Card
```

Pure.

One more stamp.

## Functions

*Pure* means the function takes no context parameter, so it cannot allocate, read, write, or observe anything — the guarantee is the absence of an argument rather than an annotation.

### call

```buri sig
fn call<C: Stdout + Allocator>(ctx: C, name: Str): ()
```

Effects: `Stdout` · `Allocator`

Says the order out loud.

### cost

```buri sig
fn cost(drink: Drink): I64
```

Pure.

What a drink costs.

### open

```buri sig
fn open(till: Till): Bool
```

Pure.

Opens the till.

Settings for the till, which has no page of its own.

### openBlank

```buri sig
fn openBlank(till: Unwritten): Bool
```

Pure.

Opens the till without being told how.

