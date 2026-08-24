# A name is declared once

```text
error: variant `Yes` is declared twice [duplicate-declaration]
```

## What to do

rename one of them; `match` tells variants apart by name

## A program that provokes it

```buri fail code=duplicate-declaration
enum Choice { Yes, No, Yes }
```
