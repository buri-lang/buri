---
title: A tool needs files in this repository
message: `{path}` is not in this repository
note: a tool is handed files from this repository alone, so every machine runs it on the same bytes
fix: check the file in, and have the tool ask for it by its repository path
reproduction: none
---
# A tool needs files in this repository

```text
error: `/etc/hosts` is not in this repository [needs-outside-repository]
 --> lib/wire/BUILD.buri:3:9
```

A tool's `check` or `generate` answers with the files it `needs`, and gets them
back in `files` on its next call. Each is a repository path:

```text
needs: ["//schemas/regions.json", "lib/wire/extra.txt"]
```

An absolute path, a URL, or a path that climbs out of the repository is
refused.
