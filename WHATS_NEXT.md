# What's Next for PDF-X?

## Current Status ✅

**All Foundation Work Complete:**
- ✅ 100% test success (141 tests passing)
- ✅ Progressive loading implemented
- ✅ Text extraction working
- ✅ Performance framework ready
- ✅ WASM support configured

## Recommended Next Steps (Priority Order)

### Phase 1: Performance Quick Wins (1-2 weeks)
**Goal:** 2-3x faster, 50% less memory
**Effort:** ~7-10 hours

1. **Replace HashMap with FxHashMap** (30 min)
   - File: `src/core/xref.rs`, `src/core/page.rs`
   - Change: `use rustc_hash::FxHashMap;`
   - Impact: 30% faster object lookups

2. **Add Inline Hints** (1 hour)
   - Files: `src/core/lexer.rs`, `src/core/parser.rs`
   - Add `#[inline]` to hot functions
   - Impact: 10-20% faster

3. **Implement Buffer Reuse in Lexer** (1 hour)
   - File: `src/core/lexer.rs`
   - Add reusable buffer to struct
   - Impact: 30% faster parsing, fewer allocations

4. **Add LRU Cache** (2 hours)
   - File: `src/core/xref.rs`
   - Replace `HashMap<u32, Rc<PDFObject>>` with `LruCache`
   - Impact: 60% memory reduction for large PDFs

5. **Use SmallVec for Arrays** (2 hours)
   - File: `src/core/parser.rs`
   - Change `Array(Vec<PDFObject>)` to `Array(SmallVec<[PDFObject; 4]>)`
   - Impact: 50% faster for small arrays (60% of cases)

6. **Run Benchmarks** (30 min)
   ```bash
   cargo bench
   ```

**See**: `PERFORMANCE.md` for detailed implementation guide

---

### Phase 2: Font Encoding Support (HIGH PRIORITY) (2-3 weeks)
**Goal:** Perfect text extraction quality
**Why:** Character-level spacing needs font metrics

**Current Issue:**
```
Output: "T race-based J ust-in-T ime"
Expected: "Trace-based Just-in-Time"
```

**What to Build:**

1. **ToUnicode CMap Parser** (1 week)
   - Parse `/ToUnicode` streams (CID → Unicode mapping)
   - File: Create `src/core/cmap.rs`
   - Reference: `pdf.js/src/core/cmap.js`

2. **CID Font Support** (1 week)
   - Handle CIDFont dictionaries
   - File: Create `src/core/font.rs`
   - Reference: `pdf.js/src/core/fonts.js`

3. **Font Metrics** (3-5 days)
   - Parse font descriptors
   - Get glyph widths for spacing calculations
   - Update text extraction to use metrics

**Impact:** Production-quality text extraction for all PDFs

**See**: `SESSION_SUMMARY.md` → Next Steps → High Priority #1

---

### Phase 3: Network Loading (2-3 weeks)
**Goal:** Load PDFs from URLs with range requests
**Why:** Core feature for web use

**What to Build:**

1. **HttpChunkedStream** (1 week)
   - Implement `BaseStream` trait for HTTP
   - Use HTTP Range headers
   - File: `src/core/http_chunked_stream.rs` (partially exists)
   - Reference: `pdf.js/src/core/chunked_stream.js`

2. **Async Support** (1 week)
   - Convert to async/await where needed
   - Use `tokio` or `async-std`
   - Keep sync API for compatibility

3. **Progress Reporting** (2-3 days)
   - Add callbacks for download progress
   - Useful for UI integration

**Example API:**
```rust
// Async API
let doc = PDFDocument::open_url("https://example.com/doc.pdf").await?;

// With progress
let doc = PDFDocument::open_url_with_progress(
    "https://example.com/doc.pdf",
    |loaded, total| {
        println!("{}% loaded", loaded * 100 / total);
    }
).await?;
```

**See**: `SESSION_SUMMARY.md` → Next Steps → High Priority #2

---

### Phase 4: Robustness & Testing (1-2 weeks)
**Goal:** Handle broken/malformed PDFs gracefully

1. **Error Recovery** (1 week)
   - Graceful degradation for corrupt PDFs
   - Better error messages with context
   - Reference: `pdf.js/src/core/parser.js` error handling

2. **Fuzzing Tests** (2-3 days)
   ```bash
   cargo install cargo-fuzz
   cargo fuzz run parse_pdf
   ```

3. **Large PDF Testing** (2-3 days)
   - Test with 100MB+ PDFs
   - Memory profiling
   - Chunk eviction testing

**See**: `SESSION_SUMMARY.md` → Next Steps → High Priority #3

---

### Phase 5: Feature Expansion (Ongoing)

**Medium Priority:**

1. **Annotations** (1-2 weeks)
   - Parse annotation dictionaries
   - Extract links, highlights, comments
   - File: Create `src/core/annotations.rs`

2. **Image Extraction** (1 week)
   - Extract embedded images from content streams
   - Decode image formats
   - File: Expand `src/core/image.rs`

3. **Metadata Extraction** (2-3 days)
   - Document info dictionary
   - XMP metadata
   - File: Add to `src/core/document.rs`

4. **Bookmarks/Outlines** (1 week)
   - Parse document outline tree
   - Build table of contents
   - File: Create `src/core/outline.rs`

**Low Priority:**

5. **Rendering** (4-6 weeks - major feature)
   - Graphics state machine
   - Path rendering
   - Text rendering
   - Would need graphics backend (tiny-skia, cairo, etc.)

6. **Form Support** (2-3 weeks)
   - Parse AcroForm fields
   - Extract form data
   - Fill forms (optional)

---

## Quick Comparison: What to Do Next?

| Task | Time | Impact | Difficulty | Priority |
|------|------|--------|------------|----------|
| **Performance Wins** | 7-10 hrs | High (2-3x faster) | Easy | ⭐⭐⭐⭐⭐ |
| **Font Encoding** | 2-3 weeks | High (perfect text) | Medium | ⭐⭐⭐⭐⭐ |
| **Network Loading** | 2-3 weeks | High (web support) | Medium | ⭐⭐⭐⭐ |
| **Robustness** | 1-2 weeks | Medium (stability) | Medium | ⭐⭐⭐⭐ |
| **Annotations** | 1-2 weeks | Medium (features) | Easy | ⭐⭐⭐ |
| **Image Extraction** | 1 week | Medium (features) | Easy | ⭐⭐⭐ |
| **Metadata** | 2-3 days | Low (nice to have) | Easy | ⭐⭐ |
| **Rendering** | 4-6 weeks | High (major feature) | Hard | ⭐⭐ |

---

## My Recommendation

### Option A: Maximum Impact, Short-Term (2-3 weeks)
```
Week 1: Performance quick wins (7-10 hrs) + Start font encoding
Week 2-3: Complete font encoding support
Result: 2-3x faster + perfect text extraction
```

### Option B: Web-First (3-4 weeks)
```
Week 1: Performance quick wins
Week 2-3: Network loading + async support
Week 4: WASM integration and testing
Result: Ready for browser deployment
```

### Option C: Incremental Improvement (Balanced)
```
Week 1: Performance quick wins
Week 2: Robustness (error handling, tests)
Week 3: One medium feature (annotations or images)
Week 4: Font encoding (start)
Result: Stable, fast, feature-rich foundation
```

---

## Starting TODAY

**If you have 30 minutes:**
```bash
# Quick win #1: FxHashMap
sed -i 's/use std::collections::HashMap/use rustc_hash::FxHashMap as HashMap/' src/core/xref.rs
cargo test
cargo bench
```

**If you have 2 hours:**
1. Implement FxHashMap (30 min)
2. Add inline hints (1 hour)
3. Run benchmarks (30 min)
4. Commit with performance improvements

**If you have 1 week:**
- Complete all Phase 1 performance wins
- Run full benchmark suite
- Document improvements
- Commit: "perf: 2-3x faster with FxHashMap, SmallVec, LRU cache"

---

## Resources

- **PERFORMANCE.md**: Detailed optimization techniques
- **WASM.md**: WebAssembly compilation guide
- **SESSION_SUMMARY.md**: Previous work summary
- **pdf.js/src/core/**: Reference implementation
- **CLAUDE.md**: Project architecture guide

---

## Decision Framework

**Choose based on your goals:**

- **Want speed now?** → Phase 1 (Performance)
- **Want perfect text?** → Phase 2 (Font encoding)
- **Want web deployment?** → Phase 3 (Network + WASM)
- **Want stability?** → Phase 4 (Robustness)
- **Want features?** → Phase 5 (Pick one)

**All phases are valuable.** I recommend **Phase 1 first** (quick wins) then **Phase 2** (font encoding) for maximum impact with reasonable effort.

Let me know which direction you want to go, and I can help you get started!
