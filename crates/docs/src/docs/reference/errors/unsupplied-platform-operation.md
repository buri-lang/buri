---
title: A platform's own operations belong to its build
message: nothing in this build supplies {operations}, which a platform declares for its own build
note: what a platform's `platform.buri` declares without a body, its `js` file implements, and only an app built for that platform hands its entry to the file; a platform's `main` is what an app's `main` matches, so nothing answers a call to it
fix: reach the effect through the host your app's `main` is handed, and give a test `platform/effect/testing`'s substitute instead
reproduction: none
---
# A platform's own operations belong to its build

```text
error: nothing in this build supplies `web.HostLocation.path`, which a platform declares for its own build [unsupplied-platform-operation]
```

A test that builds `HostLocation {}` itself has no browser behind it, on any
backend. Bind the effect to a substitute instead, such as `Location:
address(ctx, "/")` from `platform/effect/testing`.
