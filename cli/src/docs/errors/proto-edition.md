---
title: A schema declares the one edition this reader implements
message: '{declaration} is not accepted'
note: this reader implements edition {edition} and no other. One edition rather than a range is the same choice the toolchain makes everywhere else: a schema means one thing, and a reader that quietly accepted an older set of feature defaults would decode the file in front of it as a different file
fix: write `edition = "{edition}";`
reproduction: none
---
