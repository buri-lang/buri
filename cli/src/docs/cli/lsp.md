## Status

Not implemented in this toolchain. The command exists so that asking for it
gets an explanation rather than "no such command".

The analysis a language server would serve is the same one `buri build` already
runs: the front end is a library, and `driver::analyze` is the entry point a
server would call. What is missing is the protocol and the incremental
re-analysis around it, not the understanding of the code.
