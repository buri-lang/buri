This details the ecoystem features:

- JavaScript output
- Executable (macOS and Linux) output
- Test runner built in
    - Any we can easily assert changes to anything in the context without doing actual I/O operations
- LSP
- Linter built-in
- Code formatter built-in
- Mono-repo support (declaring libraries, deps, and binary build outputs, probably configured in textproto)
- Protobuf serialization / deserialization by importing a .proto file directly (does not need to be integrated into protoc, we can just do this ourselves), including json and binary serialization/deserialization
- Robust standard library
    - Networking and HTTP
    - JSON serialization/deserialization
    - UTF-8 text processing
    - Cryptography
    - Time and Date utilities
    - Collections: queues, hash maps, bit sets, simd, Struct of Arrays (MultiArrayList or something, basically the same as an array of structs but laid out differently in memory for performance reasons)
    - Multiple allocators (GeneralPurposeAllocator, arena allocator, FixedBufferAllocator)
