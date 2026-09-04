---
title: A test runs only on a platform this toolchain can build
message: no {platform} test run on this toolchain
note: {reason}, so this suite can be executed only on JavaScript
fix: drop {platform_in_build_file} from `test.platforms`, or run this suite where a {platform} artifact can be built
reproduction: none
---
