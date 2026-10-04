---
title: A dependency is visible to the package that names it
message: '{from_target} depends on {to_target}, which is not visible to it'
label: not visible
note: '{to_target} is visible to: {visible_to}'
fix: add "{from_target}" to visibility in {to_package_path}/BUILD.buri
reproduction: none
---
