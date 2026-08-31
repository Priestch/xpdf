//! Resource limits applied while decoding and resolving untrusted PDF input.
//!
//! PDF streams and object graphs are attacker-controlled.  These limits keep a
//! single stream or page from consuming unbounded memory during the resolve
//! phase. Shells may tighten them before opening untrusted documents.

/// Limits for decompression and immutable page snapshot construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Maximum bytes produced by one compressed stream.
    pub max_decompressed_bytes: usize,
    /// Maximum decompressed/compressed ratio. A one MiB floor is allowed for
    /// small, legitimately highly-compressed streams.
    pub max_compression_ratio: usize,
    /// Maximum number of indirect objects captured for one resolved page.
    pub max_resolved_objects: usize,
    /// Maximum indirect-reference/container traversal depth.
    pub max_reference_depth: usize,
    /// Maximum approximate bytes retained by one resolved page snapshot.
    pub max_resolved_page_bytes: usize,
}

impl ResourceBudget {
    /// Upper bound for a stream with `compressed_len` bytes.
    pub fn decompression_limit(&self, compressed_len: usize) -> usize {
        let ratio_limit = compressed_len
            .saturating_mul(self.max_compression_ratio)
            .max(1024 * 1024);
        self.max_decompressed_bytes.min(ratio_limit)
    }
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            max_decompressed_bytes: 256 * 1024 * 1024,
            max_compression_ratio: 4096,
            max_resolved_objects: 100_000,
            max_reference_depth: 128,
            max_resolved_page_bytes: 512 * 1024 * 1024,
        }
    }
}
