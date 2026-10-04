---
title: A generator's tool never builds from its output
message: '`{tool}` is built from {target}, which is the target that runs it'
note: 'the tool reaches back through {path}'
fix: move what the tool needs into a third library, or run the generator from a target the tool does not depend on
reproduction: none
---
# A generator's tool never builds from its output

The build links a tool before running it, and linking compiles everything the
tool depends on. That includes the rule that declared the generator, whose
modules don't exist until the tool runs. No order works.

Usually the fix is a third library: what the tool needs from the declaring
target isn't generated, so it can move somewhere both may depend on.
