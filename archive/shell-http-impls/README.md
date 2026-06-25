# Archived: HTTP chunked-stream implementations

These two files previously lived in `pdf-x-core/src/core/` behind the
`async` feature flag. They implement HTTP range-request loading using
`reqwest` + `tokio`, and were the **violation of decision A** (core must
not bake in an async runtime or do I/O).

## Why archived, not deleted

The *capability* (network progressive loading) is a core differentiator
and will be needed. But this implementation:

1. Lives in the wrong layer — decision A puts I/O in the **shell**.
   `HttpChunkedStream` constructs a `tokio::runtime::Runtime` inside the
   core, exactly what the architecture forbids.
2. Targets native only (`reqwest` is unavailable on WASM, a declared
   target). A web shell will use the `fetch` API, not this code.
3. Implements the legacy `BaseStream` interface, not the new
   `ByteSource` + `Need::Range` protocol — so it would need a rewrite
   even if it stayed.

## How to reuse

When a native shell needs HTTP loading, implement `ByteSource` (see
`pdf-x-core/src/core/byte_source.rs`) on the shell side, using these
files as an algorithm reference for:

- 64KB chunk granularity + LRU cache (delegates to `ChunkManager`)
- HTTP `Range:` header construction
- `Arc<RwLock<ChunkManager>>` sharing across sub-streams

Do not copy them back into the core. Reimplement against `ByteSource`
in the shell.

## Origin

- `async_http_chunked_stream.rs` — was `pdf-x-core/src/core/async_http_chunked_stream.rs`
- `http_chunked_stream.rs` — was `pdf-x-core/src/core/http_chunked_stream.rs`

`ChunkManager` (the LRU chunk cache they depend on) stays in the core at
`pdf-x-core/src/core/chunk_manager.rs` — it is source-agnostic and used
by `FileChunkedStream` too.
