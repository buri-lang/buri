---
title: Every alternative of an or-pattern must be reachable
message: this alternative is unreachable
note: {covered_by} already covers everything it matches
fix: delete this alternative
---
# Every alternative of an or-pattern must be reachable

```text
error: this alternative is unreachable [unreachable-alternative]
```

## What to do

Delete the alternative. The arm keeps the alternatives that still do
something, so nothing else about it changes.

## Why

`A | B` is an arm that matches two ways, and each way is a claim that some
value reaches it. An alternative the arms above already cover — or one the
alternatives to its left in the same arm already cover — is a claim that is
never true, and it is reported for the reason a whole unreachable arm is: the
usual cause is a pattern in the wrong place, and dead text in an arm reads as
handled.

An arm with no live alternative at all is one `unreachable-arm` instead, so the
two never fire on the same arm. The alternatives asked about are the arm's own,
separated by `|` at the top of its pattern; an alternation nested inside a
constructor, as in `.Some(true | false)`, counts toward coverage but is not
reported branch by branch.

## A program that provokes it

```buri fail code=unreachable-alternative
enum Hello { World, Now(Bool) }

fn greeting(h: Hello): Str {
  match (h) {
    .Now(_) => "hello world",
    .Now(_) | .World => "now",
  }
}
```
