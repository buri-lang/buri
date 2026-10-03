---
title: A `testing` module is reachable only from a test
message: this is a test-only module
label: importable only from a test source
note: a path containing a `testing` segment may be imported only from a test source
fix: import it from a file listed in a target's `test.sources`, or drop the import
---
# A `testing` module is reachable only from a test

```text
error: this is a test-only module [test-only-import]
```

```buri fail code=test-only-import
from "core/testing/assert" import * as assert;
```

Any path with a `testing` directory segment counts: `core/testing/assert`,
`//lib/ledger/testing`, `//lib/testing/fakes`. A file named `testing.buri`
doesn't, because that segment is the file's own name.
