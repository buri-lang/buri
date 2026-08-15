## Imports name the module first

```buri
from "core/list" import { map, filter };
from "core/list" import * as list;
```

The path leads so that an editor knows which module you mean before you open the
brace, and can complete the specifier list. A namespace import must be named:
bare `import *` is not derivable from the grammar, so no identifier ever enters a
module's scope without appearing in that module's own source.
