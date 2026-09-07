---
title: A generator's tool is not built from what it generates
message: '`{tool}` is built from {target}, which is the target that runs it'
note: 'the tool reaches back through {path}'
fix: move what the tool needs into a third library, or run the generator from a target the tool does not depend on
reproduction: none
---
# A generator's tool is not built from what it generates

A tool is an ordinary target. The build has to link it before it can run it, and
linking it means compiling everything it depends on — including the rule that
declared the generator, whose modules do not exist until the tool has run. There
is no order that works, so this is refused rather than attempted.

The third library is usually the answer: whatever the tool needs from the
declaring target is code the generator does not generate, and it can live
somewhere both of them may depend on.
