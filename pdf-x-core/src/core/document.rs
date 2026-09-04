use super::delta::{
    AddHighlightCommand, CommandHistory, DeletePageCommand, DeltaLayer, ReplaceTextCommand,
    RotatePageCommand, SetInfoCommand,
};
use super::doc_store::DocStore;
use super::document_loader::{DocumentLoader, OpenProgress};
use super::encryption::{EncryptDict, EncryptionVersion};
use super::error::{PDFError, PDFResult};
use super::page::{Page, PageTreeCache};
use super::parser::{PDFObject, Ref};
use super::pdf_writer::PDFWriter;
use super::stream::Stream;
use super::xref::XRef;
use lru::LruCache;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;

/// Information about a linearized PDF.
#[derive(Debug, Clone)]
pub struct LinearizedInfo {
    /// File size as specified in the linearization dictionary
    pub file_size: u64,

    /// The primary hint table offset (points to hint table location)
    pub primary_hint_offset: u64,

    /// The primary hint table length
    pub primary_hint_length: u64,

    /// The offset of the first page's object
    pub first_page_offset: u64,

    /// The number of pages
    pub page_count: u32,

    /// The object number of the first page
    pub first_page_obj_num: u32,
}

/// PDF Document reader.
///
/// This is the main entry point for reading and parsing PDF documents.
/// It handles opening a PDF file, parsing its structure, and providing
/// access to document-level information like the catalog and pages.
///
/// Based on PDF.js's PDFDocument class.
pub struct PDFDocument {
    /// The `delta → base` resolution authority. Owns the `XRef` and `DeltaLayer`.
    doc_store: DocStore,

    /// Local editor-session undo/redo state. Never serialized with the delta.
    command_history: CommandHistory,

    /// The document catalog (root dictionary)
    catalog: Option<PDFObject>,

    /// Page tree cache for efficient page lookups
    page_cache: PageTreeCache,

    /// Linearized PDF information (if applicable)
    linearized: Option<LinearizedInfo>,

    /// Encryption dictionary (if PDF is encrypted)
    encrypt_dict: Option<EncryptDict>,

    /// Cache mapping page object references to page indices
    /// Used for resolving destinations in outlines and links
    page_ref_cache: FxHashMap<(u32, u32), usize>,

    /// Whether the page reference cache has been built
    page_ref_cache_built: bool,

    /// Delta revision represented by the page-tree-derived caches.
    page_cache_revision: u64,

    /// LRU cache of resolved pages keyed by page index and delta revision.
    /// Default capacity: 10 pages (viewer rarely navigates more than ~5 pages back).
    page_resolved_cache: LruCache<(usize, u64), Arc<super::resolved_page::ResolvedPage>>,

    /// Device-specific font parse arena (type-erased). The rendering backend (e.g.
    /// tinyskia) stores `HashMap<u32, Arc<StoredFont>>` here, keyed by font object
    /// number, so parsed fonts persist across renders. Core never touches the inner
    /// type — it only holds the box.
    font_cache: Option<Box<dyn std::any::Any + Send + Sync>>,
}

/// Given a resolved page tree node, return the number of leaf pages in its subtree.
/// Returns `None` if the count cannot be determined (non-dict kid, missing `/Count`
/// on a Pages node, etc.). A single Page leaf always returns `Some(1)`.
fn get_subtree_page_count(obj: &PDFObject) -> Option<usize> {
    match obj {
        PDFObject::Dictionary(dict) => {
            if dict.contains_key("Kids") {
                // Pages intermediate node — use /Count
                dict.get("Count").and_then(|c| match c {
                    PDFObject::Number(n) if *n >= 0.0 => Some(*n as usize),
                    _ => None,
                })
            } else {
                // Page leaf node
                Some(1)
            }
        }
        _ => None,
    }
}

impl PDFDocument {
    /// Opens a PDF document from a byte array.
    ///
    /// This parses the PDF structure including the xref table and trailer,
    /// and loads the document catalog.
    ///
    /// # Arguments
    /// * `data` - The complete PDF file as bytes
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let doc = PDFDocument::open(pdf_data).unwrap();
    /// ```
    /// Shared open back half: wire decryption, load the catalog, detect
    /// linearization, and assemble the document over a parsed `XRef`.
    ///
    /// Both [`open`](Self::open) and shell-driven loader opens — and the
    /// progressive [`DocumentLoader`](super::document_loader::DocumentLoader) —
    /// converge here after their (source-specific) xref-parse phases.
    pub(crate) fn from_xref(mut xref: XRef) -> PDFResult<Self> {
        // Wire end-to-end decryption BEFORE any object fetch: read /Encrypt, auto-auth the
        // empty password, install the decryptor. (The /Encrypt dict itself and the
        // cross-reference stream are fetched/created during parse, before this point, and
        // are exempt from decryption anyway; all later fetches — catalog, pages, content —
        // must see the installed decryptor so an encrypted catalog-in-ObjStm decrypts.)
        let encrypt_dict = Self::setup_encryption(&mut xref)?;

        // Load the catalog
        let catalog = Some(xref.catalog()?);

        // Check if this is a linearized PDF
        let linearized = Self::check_linearized(&mut xref)?;

        // Delta layer over the base. Sized by the base object count so new objects
        // (Phase 2+) get fresh object numbers above the base range.
        let delta = DeltaLayer::new(xref.len() as u32);
        let doc_store = DocStore::new(xref, delta);

        Ok(PDFDocument {
            doc_store,
            command_history: CommandHistory::new(),
            catalog,
            page_cache: PageTreeCache::new(),
            linearized,
            encrypt_dict,
            page_ref_cache: FxHashMap::default(),
            page_ref_cache_built: false,
            page_cache_revision: 0,
            page_resolved_cache: LruCache::new(NonZeroUsize::new(10).unwrap()),
            font_cache: None,
        })
    }

    pub fn open(data: Vec<u8>) -> PDFResult<Self> {
        // Converged on the shell-steppable loader (Phase D step 2). A memory source is
        // fully loaded, so the open completes in one step: every range request is
        // satisfied synchronously and no `Need` can surface (equivalence-tested in
        // `loader_opens_memory_stream_identically`). The tail scan, parse, and
        // ±1KB `xref`-keyword → full-file-scan recovery ladder all live in the loader —
        // one implementation shared across shell-driven progressive opens.
        let mut loader = DocumentLoader::new(Stream::from_bytes(data));
        match loader.step()? {
            OpenProgress::Ready(doc) => Ok(*doc),
            OpenProgress::Need(need) => Err(PDFError::Generic(format!(
                "memory source must not surface a range request ({need:?})"
            ))),
        }
    }

    /// Open a PDF and authenticate with `password` when the document is encrypted.
    ///
    /// Empty-password documents still work via the auto-auth inside [`open`]; a
    /// non-empty password is applied with [`authenticate_with_password`] so the
    /// decryptor is installed before any page fetch. Returns
    /// `PDFError::ValidationError` if the password is rejected.
    ///
    /// Mirrors hayro's `Pdf::new_with_password`.
    pub fn open_with_password(data: Vec<u8>, password: &[u8]) -> PDFResult<Self> {
        let mut doc = Self::open(data)?;
        if doc.is_encrypted() && !password.is_empty() {
            let ok = doc.authenticate_with_password(password)?;
            if !ok {
                return Err(PDFError::validation_error(
                    "incorrect password for encrypted PDF",
                ));
            }
        } else if doc.is_encrypted() {
            // Empty password was already tried during open; if still no key, reject.
            if doc.get_encryption_key().is_err() {
                return Err(PDFError::validation_error(
                    "encrypted PDF requires a password",
                ));
            }
        }
        Ok(doc)
    }

    /// Open from the filesystem with progressive/chunked loading.
    ///
    /// Moved to the shell-side adapter crate in Phase D step 4 (core performs
    /// no I/O): use [`pdf_x_source_file::open_file`] —
    /// `pdf_x_source_file::open_file(path, None, None)` — which drives the same
    /// [`DocumentLoader`](super::document_loader::DocumentLoader) open with the
    /// same recovery ladder and chunked file source.

    /// Helper method to find startxref with a known offset adjustment.
    ///
    /// This is used when a tail slice has been read from the end of the file.
    pub(crate) fn find_startxref_in_bytes(data: &[u8], _base_offset: usize) -> PDFResult<usize> {
        // Find "startxref"
        let keyword = b"startxref";
        let pos = data
            .windows(keyword.len())
            .rposition(|window| window == keyword)
            .ok_or_else(|| PDFError::Generic("startxref not found in PDF".to_string()))?;

        // Skip past "startxref" and any whitespace
        let mut offset_start = pos + keyword.len();

        // Skip whitespace
        while offset_start < data.len() && data[offset_start].is_ascii_whitespace() {
            offset_start += 1;
        }

        // Read the offset number (as ASCII digits)
        let mut offset_end = offset_start;
        while offset_end < data.len() && data[offset_end].is_ascii_digit() {
            offset_end += 1;
        }

        if offset_start == offset_end {
            return Err(PDFError::Generic(
                "No offset found after startxref".to_string(),
            ));
        }

        // Parse the offset
        let offset_str = std::str::from_utf8(&data[offset_start..offset_end])
            .map_err(|_| PDFError::Generic("Invalid UTF-8 in startxref offset".to_string()))?;

        let offset: usize = offset_str
            .parse()
            .map_err(|_| PDFError::Generic("Invalid startxref offset".to_string()))?;

        Ok(offset)
    }

    /// Finds the byte offset of the cross-reference table.
    ///
    /// This searches for "startxref" near the end of the file and reads
    /// the offset that follows it.
    ///
    /// Format:
    /// ```text
    /// ...
    /// startxref
    /// 12345
    /// %%EOF
    /// ```
    fn find_startxref(data: &[u8]) -> PDFResult<usize> {
        // Search from the end of the file (last 1024 bytes)
        let search_start = if data.len() > 1024 {
            data.len() - 1024
        } else {
            0
        };

        let search_data = &data[search_start..];

        // Find "startxref"
        let keyword = b"startxref";
        let pos = search_data
            .windows(keyword.len())
            .rposition(|window| window == keyword)
            .ok_or_else(|| PDFError::Generic("startxref not found in PDF".to_string()))?;

        // Skip past "startxref" and any whitespace
        let mut offset_start = search_start + pos + keyword.len();

        // Skip whitespace
        while offset_start < data.len() && data[offset_start].is_ascii_whitespace() {
            offset_start += 1;
        }

        // Read the offset number (as ASCII digits)
        let mut offset_end = offset_start;
        while offset_end < data.len() && data[offset_end].is_ascii_digit() {
            offset_end += 1;
        }

        if offset_start == offset_end {
            return Err(PDFError::Generic(
                "No offset found after startxref".to_string(),
            ));
        }

        // Parse the offset
        let offset_str = std::str::from_utf8(&data[offset_start..offset_end])
            .map_err(|_| PDFError::Generic("Invalid UTF-8 in startxref offset".to_string()))?;

        let offset: usize = offset_str
            .parse()
            .map_err(|_| PDFError::Generic("Invalid startxref offset".to_string()))?;

        Ok(offset)
    }

    /// Returns the document catalog (root dictionary).
    pub fn catalog(&self) -> Option<&PDFObject> {
        self.catalog.as_ref()
    }

    /// Returns a mutable reference to the xref table for fetching objects.
    ///
    /// Prefer [`doc_store_mut`](Self::doc_store_mut) for new code so that edits
    /// (delta) are honored.
    pub fn xref_mut(&mut self) -> &mut XRef {
        self.doc_store.xref_mut()
    }

    /// Returns a [`DocStore`] — the single `delta → base` object-resolution authority.
    ///
    /// All object resolution should go through the returned store (rather than
    /// [`PDFDocument::xref_mut`]) so that edits are honored. This is the editor-ready
    /// resolution seam (decision B); the delta is empty in v1.
    pub fn doc_store_mut(&mut self) -> &mut DocStore {
        &mut self.doc_store
    }

    /// Execute an edit command and invalidate caches.
    fn execute_command(&mut self, cmd: Box<dyn super::delta::Command>) -> PDFResult<()> {
        if self.is_encrypted() {
            return Err(PDFError::Generic(
                "Saving edits to encrypted PDFs is not supported".into(),
            ));
        }
        self.command_history
            .execute(self.doc_store.delta_mut(), cmd, None)?;
        self.invalidate_edit_caches();
        Ok(())
    }

    /// Rotate a page by the given number of degrees (absolute, not relative).
    ///
    /// `degrees` must be a multiple of 90 (0, 90, 180, 270). This sets the
    /// `/Rotate` entry on the page dictionary to `degrees`.
    pub fn rotate_page(&mut self, page_index: usize, degrees: u16) -> PDFResult<()> {
        let page = self.get_page(page_index)?;
        let (num, generation) = page.reference().ok_or_else(|| {
            PDFError::Generic(
                "Cannot rotate a page without an indirect object reference".into(),
            )
        })?;
        let original_page = page.dict().clone();
        let cmd =
            RotatePageCommand::new(Ref::new(num, generation), degrees)
                .with_original_page(original_page);
        self.execute_command(Box::new(cmd))
    }

    /// Set entries in the document /Info dictionary.
    ///
    /// `entries` is a list of `(key, Some(value))` to set or `(key, None)` to remove.
    /// If no /Info dictionary exists, one is created automatically.
    ///
    /// Non-ASCII string values should be passed through
    /// [`SetInfoCommand::encode_info_string`] first.
    pub fn set_info(
        &mut self,
        entries: Vec<(String, Option<PDFObject>)>,
    ) -> PDFResult<()> {
        // 1. Determine the /Info ref: check delta trailer override first, then base trailer.
        let info_ref = self
            .doc_store
            .delta()
            .get_trailer_override("Info")
            .cloned()
            .or_else(|| {
                let trailer = self.xref().trailer();
                trailer
                    .as_ref()
                    .and_then(|t| match t {
                        PDFObject::Dictionary(d) => d.get("Info").cloned(),
                        _ => None,
                    })
            })
            .and_then(|v| match v {
                PDFObject::Ref(r) => Some(r),
                _ => None,
            });

        // 2. Resolve the /Info object (if the ref exists).
        //    If the ref is stale (points to a missing object), treat as None.
        let original_info = match info_ref {
            Some(ref_) => {
                match self.doc_store.resolve_ref(ref_.num, ref_.generation) {
                    Ok(obj) => Some((ref_, (*obj).clone())),
                    Err(_) => None, // stale /Info ref — create fresh
                }
            }
            None => None,
        };

        let cmd = SetInfoCommand::new(original_info, entries);
        self.execute_command(Box::new(cmd))
    }

    /// Add a standard highlight annotation in PDF user-space coordinates.
    pub fn add_highlight(
        &mut self,
        page_index: usize,
        rect: [f64; 4],
        color: [f64; 3],
    ) -> PDFResult<()> {
        let page = self.get_page(page_index)?;
        let (num, generation) = page.reference().ok_or_else(|| {
            PDFError::Generic("Cannot edit a page without an indirect object reference".into())
        })?;
        let mut page_object = page.dict().clone();
        if let PDFObject::Dictionary(dict) = &mut page_object {
            if let Some(PDFObject::Ref(reference)) = dict.get("Annots").cloned() {
                let resolved = self
                    .doc_store
                    .resolve_ref(reference.num, reference.generation)?;
                if !matches!(resolved.as_ref(), PDFObject::Array(_)) {
                    return Err(PDFError::Generic(
                        "Page Annots reference is not an array".into(),
                    ));
                }
                dict.insert("Annots".into(), resolved.as_ref().clone());
            }
        }
        let command =
            AddHighlightCommand::new(Ref::new(num, generation), page_object, rect, color)?;
        self.execute_command(Box::new(command))
    }

    /// Delete a page from the document.
    ///
    /// Removes the page from its parent's `/Kids` array and decrements `/Count`
    /// on every ancestor up to the root `/Pages` node. The page dict object is
    /// additionally marked deleted in the delta, so an incremental save
    /// persists it as a free xref entry (spec §7.5.6) and readers of the saved
    /// file can no longer resolve it.
    ///
    /// Dangling outline/GoTo destinations are tolerated by viewers (v1 limitation).
    pub fn delete_page(&mut self, page_index: usize) -> PDFResult<()> {
        // 1. Resolve the page and its parent chain.
        let page = self.get_page(page_index)?;
        let page_ref = page.reference().ok_or_else(|| {
            PDFError::Generic(
                "Cannot delete a page without an indirect object reference".into(),
            )
        })?;
        let page_ref = Ref::new(page_ref.0, page_ref.1);

        let (parent_ref, kid_index, ancestors) = self.page_parent_chain(page_index)?;

        // 2. Pre-fetch parent and ancestor dicts (avoid fetch_base in command).
        let original_parent = {
            let obj = self
                .doc_store
                .resolve_ref(parent_ref.num, parent_ref.generation)?;
            (*obj).clone()
        };

        let mut original_ancestors = Vec::new();
        for (ancestor_ref, _) in &ancestors {
            let obj = self
                .doc_store
                .resolve_ref(ancestor_ref.num, ancestor_ref.generation)?;
            original_ancestors.push((*obj).clone());
        }

        // 3. Validate the kid_index against the parent's Kids array.
        if let PDFObject::Dictionary(ref dict) = original_parent {
            if let Some(kids) = dict.get("Kids").and_then(|k| k.as_array()) {
                if kid_index >= kids.len() {
                    return Err(PDFError::Generic(format!(
                        "Internal error: kid_index {} >= Kids len {}",
                        kid_index,
                        kids.len()
                    )));
                }
            }
        }

        let cmd = DeletePageCommand::new(
            page_ref,
            parent_ref,
            kid_index,
            ancestors,
            original_parent,
            original_ancestors,
        );
        self.execute_command(Box::new(cmd))
    }

    /// Walk the page tree to find the parent of `page_index` and collect ancestors.
    ///
    /// Returns `(parent_ref, kid_index_within_parent, ancestors)` where `ancestors`
    /// is a list of `(ref, current_count)` from the immediate parent up to root `/Pages`.
    fn page_parent_chain(
        &mut self,
        page_index: usize,
    ) -> PDFResult<(Ref, usize, Vec<(Ref, u32)>)> {
        use crate::PDFObject;

        // Get the root /Pages *ref* (not resolved), so the DFS can track it.
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| PDFError::Generic("No catalog".into()))?;
        let pages_ref_obj = catalog
            .as_dictionary()
            .and_then(|d| d.get("Pages"))
            .ok_or_else(|| PDFError::Generic("No /Pages in catalog".into()))?
            .clone();
        let root_pages_ref = match &pages_ref_obj {
            PDFObject::Ref(r) => Ref::new(r.num, r.generation),
            _ => return Err(PDFError::Generic("/Pages is not an indirect reference".into())),
        };

        let mut store = self.doc_store_mut();

        // Push root as a Ref so the DFS resolves it and captures resolved_ref.
        let root_as_ref = Arc::new(PDFObject::Ref(Ref::new(
            root_pages_ref.num,
            root_pages_ref.generation,
        )));
        // Stack: (node_obj, node_ref, parent_ref)
        let mut stack: Vec<(Arc<PDFObject>, Option<Ref>, Option<Ref>)> =
            vec![(root_as_ref, None, None)];
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();
        let mut current_page_index = 0usize;

        while let Some((current_node, node_ref, parent_ref)) = stack.pop() {
            // Resolve indirect references with cycle detection.
            let (node_obj, resolved_ref) = match &*current_node {
                PDFObject::Ref(ref_obj) => {
                    let ref_key = (ref_obj.num, ref_obj.generation);
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".into(),
                        ));
                    }
                    visited_refs.insert(ref_key);
                    let obj = store.resolve_ref(ref_obj.num, ref_obj.generation)?;
                    (obj, Some(Ref::new(ref_obj.num, ref_obj.generation)))
                }
                _ => (current_node, node_ref),
            };

            let dict = match &*node_obj {
                PDFObject::Dictionary(d) => d,
                _ => continue,
            };

            let type_obj = dict.get("Type");
            let is_page = match type_obj {
                Some(PDFObject::Name(name)) => name == "Page",
                _ => !dict.contains_key("Kids"),
            };

            if is_page {
                if current_page_index == page_index {
                    // Found it. Build ancestor chain: parent + grandparents up to root.
                    let parent = parent_ref.ok_or_else(|| {
                        PDFError::Generic("Page has no parent in page tree".into())
                    })?;

                    // Walk up from parent to root via /Parent refs, collecting counts.
                    let mut ancestors: Vec<(Ref, u32)> = Vec::new();
                    let mut current = parent;
                    loop {
                        let obj = store.resolve_ref(current.num, current.generation)?;
                        let count = match &*obj {
                            PDFObject::Dictionary(d) => d
                                .get("Count")
                                .and_then(|c| c.as_number())
                                .unwrap_or(0.0) as u32,
                            _ => 0,
                        };
                        ancestors.push((current, count));

                        // Follow /Parent pointer.
                        match &*obj {
                            PDFObject::Dictionary(d) => {
                                match d.get("Parent") {
                                    Some(PDFObject::Ref(r)) => {
                                        current = Ref::new(r.num, r.generation);
                                    }
                                    _ => break, // reached root
                                }
                            }
                            _ => break,
                        }
                    }

                    // Find kid_index: which slot in parent's /Kids contains this page?
                    let target_ref = resolved_ref.unwrap();
                    let parent_obj = store.resolve_ref(parent.num, parent.generation)?;
                    let kid_index = match &*parent_obj {
                        PDFObject::Dictionary(d) => d
                            .get("Kids")
                            .and_then(|k| k.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .position(|kid| {
                                        matches!(kid.as_ref(), PDFObject::Ref(r) if r.num == target_ref.num && r.generation == target_ref.generation)
                                    })
                                    .unwrap_or(0)
                            })
                            .unwrap_or(0),
                        _ => 0,
                    };

                    return Ok((parent, kid_index, ancestors));
                }
                current_page_index += 1;
                continue;
            }

            // Pages node: descend into /Kids (reverse order to maintain index order).
            let kids = dict
                .get("Kids")
                .ok_or_else(|| PDFError::Generic("Pages node missing Kids array".into()))?;

            let kids_array = match kids {
                PDFObject::Array(arr) => arr.clone(),
                PDFObject::Ref(ref_obj) => {
                    let fetched =
                        store.resolve_ref(ref_obj.num, ref_obj.generation)?;
                    match &*fetched {
                        PDFObject::Array(arr) => arr.clone(),
                        _ => {
                            return Err(PDFError::Generic(
                                "Kids reference doesn't point to array".into(),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(PDFError::Generic(
                        "Kids is not an array or reference".into(),
                    ));
                }
            };

            let parent_for_kids = resolved_ref;
            for kid in kids_array.iter().rev() {
                stack.push((
                    Arc::new((**kid).clone()),
                    None,
                    parent_for_kids,
                ));
            }
        }

        Err(PDFError::Generic(format!(
            "Page index {} not found in page tree",
            page_index
        )))
    }

    /// Undo the most recent edit command.
    pub fn undo(&mut self) -> PDFResult<()> {
        self.command_history.undo(self.doc_store.delta_mut())?;
        self.invalidate_edit_caches();
        Ok(())
    }

    /// Redo the most recently undone edit command.
    pub fn redo(&mut self) -> PDFResult<()> {
        self.command_history.redo(self.doc_store.delta_mut())?;
        self.invalidate_edit_caches();
        Ok(())
    }

    /// Return `(dirty, can_undo, can_redo)` for editor shells.
    pub fn editor_state(&self) -> (bool, bool, bool) {
        let delta = self.doc_store.delta();
        (
            delta.change_count() > 0,
            self.command_history.can_undo(),
            self.command_history.can_redo(),
        )
    }

    /// Serialize current edits as bytes to append to `original`.
    pub fn incremental_update(&self, original: &[u8]) -> PDFResult<Vec<u8>> {
        if self.is_encrypted() {
            return Err(PDFError::Generic(
                "Saving edits to encrypted PDFs is not supported".into(),
            ));
        }
        if self.doc_store.delta().change_count() == 0 {
            return Err(PDFError::Generic("Document has no unsaved edits".into()));
        }
        let prev = Self::find_startxref(original)?;
        PDFWriter::write_incremental_update_with_trailer(
            self.doc_store.delta(),
            original.len(),
            self.doc_store.delta().next_obj_num(),
            prev,
            self.doc_store.xref().trailer(),
        )
    }

    fn invalidate_edit_caches(&mut self) {
        self.page_cache.clear();
        self.page_ref_cache.clear();
        self.page_ref_cache_built = false;
        self.page_resolved_cache.clear();
        self.page_cache_revision = self.doc_store.delta().revision();
    }

    /// Synchronize page-tree-derived caches after mutations made through the public
    /// `doc_store_mut().delta_mut()` escape hatch.
    fn sync_page_cache_revision(&mut self) -> u64 {
        let revision = self.doc_store.delta().revision();
        if revision != self.page_cache_revision {
            self.page_cache.clear();
            self.page_ref_cache.clear();
            self.page_ref_cache_built = false;
            self.page_cache_revision = revision;
        }
        revision
    }

    /// Mutable access to the device-specific font arena (type-erased).
    ///
    /// Device backends (e.g. tinyskia) initialize this on first use via
    /// [`set_font_cache`] and downcast to their concrete cache type.
    /// Core never inspects the inner type.
    pub fn font_cache_mut(&mut self) -> &mut Option<Box<dyn std::any::Any + Send + Sync>> {
        &mut self.font_cache
    }

    /// Initialize the font arena if not already set.
    pub fn set_font_cache(&mut self, arena: Box<dyn std::any::Any + Send + Sync>) {
        if self.font_cache.is_none() {
            self.font_cache = Some(arena);
        }
    }

    /// Resolve a page to an immutable, I/O-free [`ResolvedPage`] (`delta → base`).
    ///
    /// Resolve a page to an immutable `ResolvedPage`, with caching.
    ///
    /// Checks the LRU cache first; on miss, resolves through `DocStore::resolve_page`
    /// (delta → base, BFS over reachable objects), then caches the result.
    /// Cache entries are revision-scoped, including mutations made through `delta_mut()`.
    pub fn resolve_page(
        &mut self,
        page_index: usize,
    ) -> PDFResult<Arc<crate::core::resolved_page::ResolvedPage>> {
        let revision = self.sync_page_cache_revision();
        let cache_key = (page_index, revision);

        // Fast path: cache hit for this exact document revision.
        if let Some(cached) = self.page_resolved_cache.get(&cache_key) {
            return Ok(Arc::clone(cached));
        }

        // Miss: resolve and cache.
        let page = self.get_page(page_index)?;
        let resolved = Arc::new(self.doc_store_mut().resolve_page(&page)?);
        self.page_resolved_cache
            .put(cache_key, Arc::clone(&resolved));
        Ok(resolved)
    }

    /// Returns an immutable reference to the xref table.
    pub fn xref(&self) -> &XRef {
        self.doc_store.xref()
    }

    /// Gets the /Pages dictionary from the catalog.
    pub fn pages_dict(&mut self) -> PDFResult<Arc<PDFObject>> {
        let pages_ref = {
            let catalog = self
                .catalog
                .as_ref()
                .ok_or_else(|| PDFError::Generic("No catalog".to_string()))?;

            let catalog_dict = match catalog {
                PDFObject::Dictionary(dict) => dict,
                _ => return Err(PDFError::Generic("Catalog is not a dictionary".to_string())),
            };

            catalog_dict
                .get("Pages")
                .ok_or_else(|| PDFError::Generic("No /Pages in catalog".to_string()))?
                .clone()
        };

        self.doc_store.resolve_if_ref(&pages_ref)
    }

    /// Gets the page count from the /Pages dictionary.
    pub fn page_count(&mut self) -> PDFResult<u32> {
        let pages_dict = self.pages_dict()?;

        let pages_dict = match &*pages_dict {
            PDFObject::Dictionary(dict) => dict,
            _ => return Err(PDFError::Generic("/Pages is not a dictionary".to_string())),
        };

        let count = pages_dict
            .get("Count")
            .ok_or_else(|| PDFError::Generic("No /Count in /Pages".to_string()))?;

        match count {
            PDFObject::Number(n) => Ok(*n as u32),
            _ => Err(PDFError::Generic("/Count is not a number".to_string())),
        }
    }

    /// Builds the page reference cache by traversing the entire page tree.
    ///
    /// This method walks all pages in the document and creates a mapping from
    /// page object references (obj_num, generation) to page indices. This cache
    /// is used by outlines and links to resolve page references to page indices.
    ///
    /// The cache is built lazily - only when first needed (e.g., when parsing outlines).
    fn build_page_ref_cache(&mut self) -> PDFResult<()> {
        if self.page_ref_cache_built {
            return Ok(());
        }

        // Get the root Pages dictionary
        let root_pages = self.pages_dict()?;

        // Stack for depth-first traversal: (node, node_ref)
        let mut nodes_to_visit: Vec<(Arc<PDFObject>, Option<(u32, u32)>)> =
            vec![(root_pages, None)];
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();
        let mut current_page_index = 0;

        while let Some((current_node, node_ref)) = nodes_to_visit.pop() {
            // Handle references
            let (node_obj, obj_ref) = match &*current_node {
                PDFObject::Ref(ref_obj) => {
                    let ref_key = (ref_obj.num, ref_obj.generation);

                    // Prevent circular references
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".to_string(),
                        ));
                    }
                    visited_refs.insert(ref_key);

                    // Fetch the object
                    let obj = self
                        .doc_store
                        .resolve_ref(ref_obj.num, ref_obj.generation)?;
                    (obj, Some(ref_key))
                }
                _ => (current_node, node_ref),
            };

            // Check if this is a dictionary
            let dict = match &*node_obj {
                PDFObject::Dictionary(d) => d,
                _ => continue,
            };

            // Check the Type field
            let type_obj = dict.get("Type");
            let is_page = match type_obj {
                Some(PDFObject::Name(name)) => name == "Page",
                _ => !dict.contains_key("Kids"), // If no Type, check for Kids (leaf node)
            };

            if is_page {
                // This is a page node - cache its reference
                if let Some(ref_key) = obj_ref {
                    self.page_ref_cache.insert(ref_key, current_page_index);
                }
                current_page_index += 1;
                continue;
            }

            // This is an intermediate Pages node - traverse its Kids
            let kids = dict
                .get("Kids")
                .ok_or_else(|| PDFError::Generic("Pages node missing Kids array".to_string()))?;

            // Get the kids array (either directly or by resolving a reference)
            let kids_array = match kids {
                PDFObject::Array(arr) => arr.clone(),
                PDFObject::Ref(ref_obj) => {
                    // Kids is a reference, fetch it
                    let fetched = self
                        .doc_store
                        .resolve_ref(ref_obj.num, ref_obj.generation)?;
                    match &*fetched {
                        PDFObject::Array(arr) => arr.clone(),
                        _ => {
                            return Err(PDFError::Generic(
                                "Kids reference doesn't point to array".to_string(),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(PDFError::Generic(
                        "Kids is not an array or reference".to_string(),
                    ));
                }
            };

            // Add kids to stack in reverse order (to maintain order during DFS)
            for kid in kids_array.iter().rev() {
                nodes_to_visit.push((Arc::new((**kid).clone()), None));
            }
        }

        self.page_ref_cache_built = true;
        Ok(())
    }

    /// Resolves a page reference to a page index.
    ///
    /// This method looks up a page's object reference in the page reference cache
    /// and returns the corresponding page index. Returns None if the reference
    /// is not found in the cache.
    ///
    /// # Arguments
    /// * `ref_num` - The object number of the page reference
    /// * `ref_gen` - The generation number of the page reference
    ///
    /// # Returns
    /// * `Some(page_index)` - The page index if found
    /// * `None` - If the reference is not in the cache
    pub fn resolve_page_index(&mut self, ref_num: u32, ref_gen: u32) -> Option<usize> {
        // Ensure cache is built
        if !self.page_ref_cache_built {
            let _ = self.build_page_ref_cache();
        }

        self.page_ref_cache.get(&(ref_num, ref_gen)).copied()
    }

    /// Traverses the page tree to find a specific page by index, using count-based
    /// branch skipping to avoid O(N²) on sequential access.
    ///
    /// PDF page trees can be hierarchical with intermediate "Pages" nodes
    /// containing "Kids" arrays. Each Pages node has a `/Count` field giving the
    /// total number of leaf pages in its subtree. When iterating Kids, if the
    /// entire subtree lies before the target index, we skip it without entering
    /// — this makes each call visit O(depth) nodes rather than O(N).
    ///
    /// Mirrors PDF.js's `Catalog.getPageDict` approach adapted for count-based
    /// pruning.
    ///
    /// Cache: found pages are cached by ref in `PageTreeCache.page_refs`.
    /// `fully_built` is only set when the tree has been exhausted (page not found),
    /// preventing repeated failed traversals for out-of-bounds requests.
    fn get_page_dict(
        &mut self,
        page_index: usize,
    ) -> PDFResult<(Arc<PDFObject>, Option<(u32, u32)>)> {
        // Fast path: check the forward ref cache.
        let cached_target = self.page_cache.get_page_ref(page_index);
        if let Some(cached_ref) = cached_target {
            let mut store = self.doc_store_mut();
            let page_dict = store.resolve_ref(cached_ref.0, cached_ref.1)?;
            return Ok((page_dict, Some(cached_ref)));
        }

        // Already fully walked but page_index not cached — doesn't exist.
        if self.page_cache.is_fully_built() {
            return Err(PDFError::Generic(format!(
                "Page index {} not found in page tree",
                page_index
            )));
        }

        // Slow path: DFS with count-based branch skipping.
        let root_pages = self.pages_dict()?;
        let mut store = self.doc_store_mut();

        let mut stack: Vec<(Arc<PDFObject>, Option<(u32, u32)>)> = vec![(root_pages, None)];
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();
        let mut current_page_index = 0usize;
        let mut discovered: Vec<(usize, Option<(u32, u32)>)> = Vec::new();

        while let Some((current_node, node_ref)) = stack.pop() {
            // Resolve indirect references with cycle detection.
            let (node_obj, resolved_ref) = match &*current_node {
                PDFObject::Ref(ref_obj) => {
                    let ref_key = (ref_obj.num, ref_obj.generation);
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".to_string(),
                        ));
                    }
                    visited_refs.insert(ref_key);
                    let obj = store.resolve_ref(ref_obj.num, ref_obj.generation)?;
                    (obj, Some(ref_key))
                }
                _ => (current_node, node_ref),
            };

            let dict = match &*node_obj {
                PDFObject::Dictionary(d) => d,
                _ => continue,
            };

            let type_obj = dict.get("Type");
            let is_page = match type_obj {
                Some(PDFObject::Name(name)) => name == "Page",
                _ => !dict.contains_key("Kids"),
            };

            if is_page {
                // Record every page we pass through (builds cache for future lookups).
                discovered.push((current_page_index, resolved_ref));

                if current_page_index == page_index {
                    // Found target — flush cache and return.
                    drop(store);
                    for (idx, ref_opt) in &discovered {
                        self.page_cache.put_page_ref(*idx, *ref_opt);
                    }
                    return Ok((node_obj, resolved_ref));
                }

                current_page_index += 1;
                continue;
            }

            // Pages node: get Kids array.
            let kids = dict
                .get("Kids")
                .ok_or_else(|| PDFError::Generic("Pages node missing Kids array".to_string()))?;

            let kids_array = match kids {
                PDFObject::Array(arr) => arr.clone(),
                PDFObject::Ref(ref_obj) => {
                    let fetched = store.resolve_ref(ref_obj.num, ref_obj.generation)?;
                    match &*fetched {
                        PDFObject::Array(arr) => arr.clone(),
                        _ => {
                            return Err(PDFError::Generic(
                                "Kids reference doesn't point to array".to_string(),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(PDFError::Generic(
                        "Kids is not an array or reference".to_string(),
                    ));
                }
            };

            // Walk kids in forward order, accumulating page count.
            // For each kid: if `base + count > page_index` the target is inside
            // this kid's subtree — enter it. Otherwise add `count` to `base`
            // and skip the entire branch. Unknown count forces entry (can't skip).
            let mut base = current_page_index;
            let mut entered_kid = false;

            for kid_ref_obj in kids_array.iter() {
                let kid = &**kid_ref_obj;

                let (resolved_kid, kid_ref, subtree_count) = match kid {
                    PDFObject::Ref(ref_obj) => {
                        let ref_key = (ref_obj.num, ref_obj.generation);
                        let resolved = store.resolve_ref(ref_obj.num, ref_obj.generation)?;
                        let count = get_subtree_page_count(&*resolved);
                        (resolved, Some(ref_key), count)
                    }
                    _ => {
                        let count = get_subtree_page_count(kid);
                        (Arc::new(kid.clone()), None, count)
                    }
                };

                let should_enter = match subtree_count {
                    Some(c) => base + c > page_index,
                    None => true, // unknown count — must enter
                };

                if should_enter {
                    if let Some(ref_key) = kid_ref {
                        if visited_refs.contains(&ref_key) {
                            return Err(PDFError::Generic(
                                "Circular reference in page tree".to_string(),
                            ));
                        }
                        visited_refs.insert(ref_key);
                    }
                    current_page_index = base;
                    stack.push((resolved_kid, kid_ref));
                    entered_kid = true;
                    break;
                }

                if let Some(c) = subtree_count {
                    base += c;
                }
            }

            if !entered_kid {
                // Every kid was skipped — update position past this entire subtree.
                current_page_index = base;
            }
        }

        // Exhausted the tree without finding the page.
        drop(store);
        self.page_cache.set_fully_built();
        Err(PDFError::Generic(format!(
            "Page index {} not found in page tree",
            page_index
        )))
    }

    /// Gets a specific page by index (0-based).
    ///
    /// Pages are loaded lazily and cached. The first call for a page will
    /// traverse the page tree; subsequent calls return the cached page.
    ///
    /// # Arguments
    /// * `page_index` - The 0-based page index (0 = first page)
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// let pdf_data = std::fs::read("document.pdf").unwrap();
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    ///
    /// // Get the first page
    /// let page = doc.get_page(0).unwrap();
    /// println!("Page index: {}", page.index());
    /// ```
    pub fn get_page(&mut self, page_index: usize) -> PDFResult<Page> {
        self.sync_page_cache_revision();

        // Check cache first
        if let Some(cached_page) = self.page_cache.get(page_index) {
            return Ok(cached_page.clone());
        }

        // Traverse the page tree to find the page
        let (page_dict, page_ref) = self.get_page_dict(page_index)?;

        // Create the Page object
        let page = Page::new(page_index, page_dict, page_ref);

        // Cache it
        self.page_cache.put(page_index, page.clone());

        Ok(page)
    }

    /// Extracts text from a specific page.
    ///
    /// # Arguments
    /// * `page_index` - Zero-based page index
    ///
    /// # Returns
    /// A vector of TextItem objects containing text content with position and font info
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let text_items = doc.extract_text_from_page(0).unwrap();
    /// for item in text_items {
    ///     println!("Text: {} at {:?}", item.text, item.position);
    /// }
    /// ```
    pub fn extract_text_from_page(
        &mut self,
        page_index: usize,
    ) -> PDFResult<Vec<crate::core::content_stream::TextItem>> {
        let page = self.get_page(page_index)?;
        page.extract_text(self.doc_store_mut())
    }

    /// Extracts text from a page as a single string.
    ///
    /// This is a convenience method that joins all text items together.
    ///
    /// # Arguments
    /// * `page_index` - Zero-based page index
    ///
    /// # Returns
    /// The extracted text as a single string
    pub fn extract_text_from_page_as_string(&mut self, page_index: usize) -> PDFResult<String> {
        let page = self.get_page(page_index)?;
        page.extract_text_as_string(self.doc_store_mut())
    }

    /// Find `needle` as an exact Unicode substring on a page.
    ///
    /// Returns the first match (or the N-th if `opts.occurrence` is set).
    /// v1 scope: matches within a single Tj/TJ operation only.
    pub fn find_text_on_page(
        &mut self,
        page_index: usize,
        needle: &str,
        opts: &super::content_edit::FindOptions,
    ) -> PDFResult<Option<super::content_edit::TextMatch>> {
        use super::content_edit::{ContentFragment, EditableContentStream, TextRunLocator};
        use super::content_stream::load_fonts_from_resources;
        use super::decode;

        let page = self.get_page(page_index)?;
        let store = self.doc_store_mut();

        // Resolve /Contents (single stream or array).
        let contents_obj = match page.contents() {
            Some(c) => store.resolve_if_ref(c)?,
            None => return Ok(None),
        };

        let content_streams: Vec<(std::collections::HashMap<String, PDFObject>, Vec<u8>)> = match &*contents_obj {
            PDFObject::Stream { dict, data } => vec![(dict.clone(), data.clone())],
            PDFObject::Array(arr) => {
                let mut out = Vec::new();
                for item in arr.iter() {
                    match &*store.resolve_if_ref(item)? {
                        PDFObject::Stream { dict, data } => {
                            out.push((dict.clone(), data.clone()));
                        }
                        _ => return Err(PDFError::Generic(
                            "Contents array contains non-stream object".into(),
                        )),
                    }
                }
                out
            }
            _ => return Ok(None),
        };

        // Decode each fragment and build ContentFragment metadata.
        let mut fragment_pairs: Vec<(Vec<u8>, ContentFragment)> = Vec::new();
        let mut decoded_offset = 0usize;
        for (dict, raw_data) in &content_streams {
            let (decoded, filter_name) = if let Some(filter) = dict.get("Filter") {
                let name = match filter {
                    PDFObject::Name(n) => Some(n.clone()),
                    _ => None,
                };
                let decoded = decode::apply_filters(raw_data, filter)
                    .map_err(|_| PDFError::Generic("failed to decode content stream".into()))?;
                (decoded, name)
            } else {
                (raw_data.to_vec(), None)
            };
            let has_decode_parms = dict.get("DecodeParms").is_some();
            let frag = ContentFragment {
                filter_name,
                has_decode_parms,
                raw: raw_data.clone(),
                decoded_offset,
                decoded_len: decoded.len(),
            };
            decoded_offset += decoded.len();
            fragment_pairs.push((decoded, frag));
        }

        let ecs = EditableContentStream::build(&fragment_pairs)?;

        // Load fonts from page resources.
        let page_resources = page.get_inheritable_resources(store)?;
        let fonts: FxHashMap<String, super::font::Font> = match page_resources {
            Some(ref res) => load_fonts_from_resources(&**res, store),
            None => FxHashMap::default(),
        };

        let locator = TextRunLocator::new(&ecs, &fonts);
        Ok(locator.find_text(needle, opts))
    }

    /// Replace `find` with `replace` on `page_index`.
    ///
    /// Only the first match is replaced (use `FindOptions::occurrence` for
    /// subsequent ones).  v1 constraints:
    /// - Single-op match only (no cross-Tj/TJ replacement).
    /// - CID fonts are rejected.
    /// - `'` / `"` operators are rejected.
    /// - Only bare /FlateDecode or no filter supported.
    pub fn replace_text(
        &mut self,
        page_index: usize,
        find: &str,
        replace: &str,
        opts: &super::content_edit::FindOptions,
    ) -> PDFResult<()> {
        use super::content_edit::{
            ContentFragment, EditableContentStream, TextRunLocator,
            serialize_hex_string, serialize_operation,
        };
        use super::content_stream::{load_fonts_from_resources, OpCode};
        use super::decode;

        let page = self.get_page(page_index)?;
        let store = self.doc_store_mut();

        // ── 1. Resolve /Contents with refs ────────────────────────────
        let contents_raw = match page.contents() {
            Some(c) => c.clone(),
            None => return Ok(()), // nothing to replace
        };

        let contents_obj = store.resolve_if_ref(&contents_raw)?;

        // (ref, stream_dict, raw_data) per fragment.
        // ref is None for direct objects (cannot be delta-modified).
        let mut stream_info: Vec<(Option<Ref>, std::collections::HashMap<String, PDFObject>, Vec<u8>)> =
            Vec::new();

        match &*contents_obj {
            PDFObject::Stream { dict, data } => {
                let ref_ = Self::obj_ref(&contents_raw);
                stream_info.push((ref_, dict.clone(), data.clone()));
            }
            PDFObject::Array(arr) => {
                for item in arr.iter() {
                    let ref_ = Self::obj_ref(item);
                    match &*store.resolve_if_ref(item)? {
                        PDFObject::Stream { dict, data } => {
                            stream_info.push((ref_, dict.clone(), data.clone()));
                        }
                        _ => {
                            return Err(PDFError::Generic(
                                "Contents array contains non-stream object".into(),
                            ))
                        }
                    }
                }
            }
            _ => return Ok(()),
        }

        // ── 2. Build EditableContentStream ────────────────────────────
        let mut fragment_pairs: Vec<(Vec<u8>, ContentFragment)> = Vec::new();
        let mut decoded_offset = 0usize;
        for (_ref, dict, raw_data) in &stream_info {
            let (decoded, filter_name) = if let Some(filter) = dict.get("Filter") {
                let name = match filter {
                    PDFObject::Name(n) => Some(n.clone()),
                    _ => None,
                };
                let decoded = decode::apply_filters(raw_data, filter)
                    .map_err(|_| PDFError::Generic("failed to decode content stream".into()))?;
                (decoded, name)
            } else {
                (raw_data.to_vec(), None)
            };
            let has_decode_parms = dict.get("DecodeParms").is_some();
            let frag = ContentFragment {
                filter_name,
                has_decode_parms,
                raw: raw_data.clone(),
                decoded_offset,
                decoded_len: decoded.len(),
            };
            decoded_offset += decoded.len();
            fragment_pairs.push((decoded, frag));
        }
        let mut ecs = EditableContentStream::build(&fragment_pairs)?;

        // ── 3. Load fonts ─────────────────────────────────────────────
        let page_resources = page.get_inheritable_resources(store)?;
        let fonts: FxHashMap<String, super::font::Font> = match page_resources {
            Some(ref res) => load_fonts_from_resources(&**res, store),
            None => FxHashMap::default(),
        };

        // ── 4. Find the match ─────────────────────────────────────────
        let locator = TextRunLocator::new(&ecs, &fonts);
        let m = match locator.find_text(find, opts) {
            Some(m) => m,
            None => return Ok(()), // nothing found — no-op
        };

        // ── 5. Get the font for this op and build inverse map ─────────
        let op = &ecs.ops[m.op_index];
        let op_code = op.op.as_ref().ok_or_else(|| {
            PDFError::Generic("matched operation has no operator".into())
        })?;
        match op_code {
            OpCode::ShowText => {}
            _ => {
                return Err(PDFError::Generic(
                    "v1 only supports replacing Tj operators".into(),
                ))
            }
        }

        // Find the font by scanning backward from the matched op.
        let font_name = Self::find_font_name_for_op(&ecs, m.op_index).ok_or_else(|| {
            PDFError::Generic("no font found for matched text operation".into())
        })?;
        let font = fonts.get(&font_name).ok_or_else(|| {
            PDFError::Generic(format!("font \"{font_name}\" not found in page resources"))
        })?;

        if font.font_type().is_cid_font() {
            return Err(PDFError::Generic(
                "v1 does not support text replacement in CID fonts".into(),
            ));
        }
        let inverse = font.build_inverse_map().ok_or_else(|| {
            PDFError::Generic("failed to build inverse encoding map".into())
        })?;
        if !font.can_encode(&inverse, replace) {
            return Err(PDFError::Generic(
                "replacement text contains characters not encodable in this font".into(),
            ));
        }

        // ── 6. Build full replacement string (prefix + replace + suffix) ──
        // Get the full decoded text of the matched Tj op.
        let locator2 = TextRunLocator::new(&ecs, &fonts);
        let run = locator2
            .collect_runs()
            .into_iter()
            .find(|r| r.op_index == m.op_index)
            .ok_or_else(|| {
                PDFError::Generic("matched op_index has no text run".into())
            })?;
        let chars: Vec<char> = run.decoded.chars().collect();
        let prefix: String = chars[..m.char_range.start].iter().collect();
        let suffix: String = chars[m.char_range.end..].iter().collect();
        let full_new_text = format!("{prefix}{replace}{suffix}");

        // ── 7. Encode the full new string as glyph codes ─────────────────
        if !font.can_encode(&inverse, &full_new_text) {
            return Err(PDFError::Generic(
                "full replacement string contains characters not encodable in this font".into(),
            ));
        }
        let mut codes: Vec<u8> = Vec::with_capacity(full_new_text.len());
        for ch in full_new_text.chars() {
            let code = inverse[&ch] as u8;
            codes.push(code);
        }

        // ── 8. Splice replacement into ECS ────────────────────────────
        let new_op = serialize_operation(OpCode::ShowText, &serialize_hex_string(&codes));
        ecs.replace_op_bytes(m.op_index, new_op);

        // ── 9. Serialize modified fragments ────────────────────────────
        let serialized = ecs.serialize_fragments()?;

        // ── 10. Build original + replacement stream objects ────────────
        // Only fragments that were modified need to be in the command.
        // For simplicity, include all fragments that belong to the affected stream_index.
        let affected_stream_index = ecs.ops[m.op_index].stream_index;
        let affected_ref = stream_info[affected_stream_index].0.ok_or_else(|| {
            PDFError::Generic(
                "content stream is a direct object; cannot modify via delta".into(),
            )
        })?;

        // Build original stream object for the affected fragment.
        let orig_dict = stream_info[affected_stream_index].1.clone();
        let orig_data = stream_info[affected_stream_index].2.clone();
        let original_stream = PDFObject::Stream {
            dict: orig_dict,
            data: orig_data,
        };

        // Build replacement stream object.
        let (new_data, new_filter) = &serialized[affected_stream_index];
        let mut new_dict = stream_info[affected_stream_index].1.clone();
        match new_filter {
            Some(f) => {
                new_dict.insert("Filter".to_string(), PDFObject::Name(f.clone()));
            }
            None => {
                new_dict.remove("Filter");
                new_dict.remove("DecodeParms");
            }
        }
        new_dict.insert(
            "Length".to_string(),
            PDFObject::Number(new_data.len() as f64),
        );
        let replacement_stream = PDFObject::Stream {
            dict: new_dict,
            data: new_data.clone(),
        };

        // ── 11. Create and execute the command ────────────────────────
        let cmd = ReplaceTextCommand::new(vec![(
            affected_ref,
            original_stream,
            replacement_stream,
        )]);
        self.execute_command(Box::new(cmd))
    }

    /// Extract the `Ref` from a `PDFObject::Reference`, if any.
    fn obj_ref(obj: &PDFObject) -> Option<Ref> {
        match obj {
            PDFObject::Ref(r) => Some(*r),
            _ => None,
        }
    }

    /// Find the font name for a text-showing op by scanning backward for Tf.
    fn find_font_name_for_op(
        ecs: &super::content_edit::EditableContentStream,
        op_index: usize,
    ) -> Option<String> {
        use super::content_edit::parse_tf_args;
        for prev_op in ecs.ops[..op_index].iter().rev() {
            if let Some(super::content_stream::OpCode::SetFont) = prev_op.op {
                let span_bytes = &ecs.bytes()[prev_op.span.clone()];
                if let Some((name, _)) = parse_tf_args(span_bytes) {
                    return Some(name);
                }
                break;
            }
        }
        None
    }

    /// Gets an inheritable property from a page dictionary.
    ///
    /// PDF pages can inherit certain properties from parent Pages nodes in the
    /// page tree. This method walks up the tree following "Parent" references
    /// until it finds the property.
    ///
    /// Inheritable properties include:
    /// - Resources: Fonts, images, and other resources
    /// - MediaBox: Page dimensions
    /// - CropBox: Visible page area
    /// - Rotate: Page rotation
    ///
    /// # Arguments
    /// * `page` - The page to start searching from
    /// * `key` - The property name (e.g., "MediaBox", "Resources")
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];  // Placeholder for example
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    ///
    /// // Get MediaBox (might be inherited from parent)
    /// let media_box = doc.get_inheritable_property(&page, "MediaBox").ok();
    /// ```
    pub fn get_inheritable_property(
        &mut self,
        page: &Page,
        key: &str,
    ) -> PDFResult<Arc<PDFObject>> {
        let mut current_dict = page.dict().clone();
        let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();

        loop {
            // Get the dictionary
            let dict = match &current_dict {
                PDFObject::Dictionary(d) => d,
                _ => return Err(PDFError::Generic("Not a dictionary".to_string())),
            };

            // Check if this dictionary has the property
            if let Some(value) = dict.get(key) {
                // Found it! Resolve if it's a reference
                return self.doc_store.resolve_if_ref(value);
            }

            // Not found, try parent
            let parent = dict.get("Parent").ok_or_else(|| {
                PDFError::Generic(format!("Property '{}' not found in page tree", key))
            })?;

            // Resolve parent if it's a reference
            match parent {
                PDFObject::Ref(ref_obj) => {
                    let ref_key = (ref_obj.num, ref_obj.generation);

                    // Prevent circular references
                    if visited_refs.contains(&ref_key) {
                        return Err(PDFError::Generic(
                            "Circular reference in page tree".to_string(),
                        ));
                    }
                    visited_refs.insert(ref_key);

                    // Fetch the parent dictionary
                    let parent_obj = self
                        .doc_store
                        .resolve_ref(ref_obj.num, ref_obj.generation)?;
                    current_dict = (*parent_obj).clone();
                }
                _ => {
                    // Parent is not a reference, use it directly
                    current_dict = parent.clone();
                }
            }
        }
    }

    /// Gets the MediaBox for a page, using inheritance if needed.
    ///
    /// MediaBox defines the boundaries of the physical medium on which the page
    /// is to be printed. It's an array of 4 numbers: [llx, lly, urx, ury]
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];  // Placeholder for example
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let media_box = doc.get_media_box(&page).unwrap();
    /// ```
    pub fn get_media_box(&mut self, page: &Page) -> PDFResult<Arc<PDFObject>> {
        self.get_inheritable_property(page, "MediaBox")
    }

    /// Gets the Resources dictionary for a page, using inheritance if needed.
    ///
    /// Resources contains fonts, images, and other resources used by the page.
    ///
    /// # Example
    /// ```no_run
    /// use pdf_x_core::core::PDFDocument;
    ///
    /// # let pdf_data = vec![];  // Placeholder for example
    /// let mut doc = PDFDocument::open(pdf_data).unwrap();
    /// let page = doc.get_page(0).unwrap();
    /// let resources = doc.get_resources(&page).ok();
    /// ```
    pub fn get_resources(&mut self, page: &Page) -> PDFResult<Arc<PDFObject>> {
        self.get_inheritable_property(page, "Resources")
    }

    /// Gets the CropBox for a page, using inheritance if needed.
    ///
    /// CropBox defines the region to which the contents of the page should be
    /// clipped when displayed or printed.
    pub fn get_crop_box(&mut self, page: &Page) -> PDFResult<Arc<PDFObject>> {
        self.get_inheritable_property(page, "CropBox")
    }

    /// Gets the Rotate value for a page, using inheritance if needed.
    ///
    /// Rotate specifies the number of degrees by which the page should be
    /// rotated clockwise when displayed or printed. Must be a multiple of 90.
    pub fn get_rotate(&mut self, page: &Page) -> PDFResult<Arc<PDFObject>> {
        self.get_inheritable_property(page, "Rotate")
    }

    /// Read `/Encrypt` from the trailer, build an [`EncryptDict`], and auto-authenticate
    /// the empty user password (the common corpus case). If the key is derived, install a
    /// decryptor on `xref` so subsequent fetches decrypt streams/strings inline; in either
    /// case return the (possibly un-keyed) `EncryptDict` so `is_encrypted()` etc. work.
    /// Mirrors pdf.js building a `CipherTransformFactory` in `XRef.parse`.
    fn setup_encryption(xref: &mut XRef) -> PDFResult<Option<EncryptDict>> {
        let Some(encrypt_ref) = xref.get_encrypt_dict_ref() else {
            return Ok(None);
        };
        // Fetch the /Encrypt dict BEFORE installing any decryptor (it is never encrypted).
        let encrypt_obj = xref.fetch_if_ref(&encrypt_ref)?;
        let mut encrypt = EncryptDict::from_object(&encrypt_obj)?;

        // Record the /Encrypt object number so fetch can exempt it from decryption.
        let encrypt_obj_num = match &encrypt_ref {
            PDFObject::Ref(r) => Some(r.num),
            _ => None,
        };

        // Auto-authenticate the empty user password (most test PDFs use it).
        let keyed = match encrypt.encryption_version() {
            EncryptionVersion::V5R5 | EncryptionVersion::V5R6 => encrypt.check_user_password(b""),
            _ => {
                // Legacy (V1/2/4) needs the file ID from the trailer.
                match xref.file_id() {
                    Ok(file_id) => encrypt.derive_encryption_key_with_file_id(b"", &file_id),
                    Err(_) => false,
                }
            }
        };
        if keyed {
            xref.set_decryptor(Some(encrypt.clone()), encrypt_obj_num);
        }
        Ok(Some(encrypt))
    }

    /// Checks if this PDF is linearized (optimized for web view).
    ///
    /// Linearized PDFs (also known as "optimized for web" or "fast web view")
    /// allow the first page to be displayed before the entire file is downloaded.
    ///
    /// # Returns
    /// `Some(LinearizedInfo)` if the PDF is linearized, `None` otherwise
    ///
    /// Based on PDF.js's checkLinearization method
    fn check_linearized(xref: &mut XRef) -> PDFResult<Option<LinearizedInfo>> {
        // Linearized PDFs have an object at the beginning of the file
        // with /Linearized in the dictionary. Usually object 1.

        // Try to get object 1 (common position for linearization dict)
        let obj1_result = xref.fetch(1, 0);

        let obj1 = match obj1_result {
            Ok(obj) => obj,
            Err(_) => {
                // Object 1 doesn't exist or can't be fetched - not linearized
                return Ok(None);
            }
        };

        // Check if it's a dictionary with /Linearized entry
        let dict = match &*obj1 {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(None),
        };

        // Check for /Linearized key
        let _linearized_version = match dict.get("Linearized") {
            Some(PDFObject::Number(n)) => *n,
            _ => return Ok(None),
        };

        // Linearization is an optimization, not a structural requirement: a missing or
        // malformed linearization dict must NOT prevent the document from opening. Treat the
        // whole field extraction as best-effort — if we can't build full LinearizedInfo, we
        // simply report "not usable as linearized" (Ok(None)) and fall back to normal access.
        let as_num = |key: &str| -> Option<u64> {
            match dict.get(key)? {
                PDFObject::Number(n) => Some(*n as u64),
                // Some generators emit the first-page entry as an indirect ref (`/O 2 0 R`)
                // rather than the bare object number — accept both.
                PDFObject::Ref(r) => Some(r.num as u64),
                _ => None,
            }
        };

        // /O (first-page object number) and /N (page count) are the load-bearing fields for
        // fast first paint; the rest are hints. Require at least these two to report
        // linearization, else degrade gracefully.
        let first_page_obj_num = match as_num("O") {
            Some(n) => n as u32,
            None => return Ok(None),
        };
        let page_count = match as_num("N") {
            Some(n) => n as u32,
            None => return Ok(None),
        };

        let file_size = as_num("L").unwrap_or(0);
        let first_page_offset = first_page_obj_num as u64; // object number; offset resolved lazily
        let first_page_p = match dict.get("P") {
            Some(PDFObject::Number(n)) => *n as u32,
            Some(PDFObject::Ref(r)) => r.num,
            _ => first_page_obj_num,
        };
        let (primary_hint_offset, primary_hint_length) = match dict.get("H") {
            Some(PDFObject::Array(arr)) if arr.len() >= 2 => {
                let off = match &*arr[0] {
                    PDFObject::Number(n) => *n as u64,
                    _ => 0,
                };
                let len = match &*arr[1] {
                    PDFObject::Number(n) => *n as u64,
                    _ => 0,
                };
                (off, len)
            }
            _ => (0, 0),
        };

        Ok(Some(LinearizedInfo {
            file_size,
            primary_hint_offset,
            primary_hint_length,
            first_page_offset,
            page_count,
            first_page_obj_num: first_page_p,
        }))
    }

    /// Returns information about linearized PDF optimization, if available.
    ///
    /// # Returns
    /// `Some(&LinearizedInfo)` if the PDF is linearized, `None` otherwise
    pub fn linearized_info(&self) -> Option<&LinearizedInfo> {
        self.linearized.as_ref()
    }

    /// Returns true if this PDF is linearized (optimized for web view).
    pub fn is_linearized(&self) -> bool {
        self.linearized.is_some()
    }

    /// Returns true if this PDF is encrypted and requires a password.
    pub fn is_encrypted(&self) -> bool {
        self.encrypt_dict.is_some()
    }

    /// Gets the encryption dictionary for this PDF (if encrypted).
    pub fn encrypt_dict(&self) -> Option<&EncryptDict> {
        self.encrypt_dict.as_ref()
    }

    /// Checks if a user password is correct and derives the encryption key.
    ///
    /// This method should only be called for PDF 2.0 (V=5) encrypted PDFs.
    /// For earlier versions, use `authenticate_with_password` instead.
    ///
    /// # Arguments
    /// * `password` - The password to check (as UTF-8 bytes)
    ///
    /// # Returns
    /// `true` if the password is correct and the encryption key was derived
    pub fn check_user_password(&mut self, password: &[u8]) -> bool {
        if let Some(ref mut encrypt_dict) = self.encrypt_dict {
            encrypt_dict.check_user_password(password)
        } else {
            false // Not encrypted
        }
    }

    /// Checks if an owner password is correct and derives the encryption key.
    ///
    /// This method should only be called for PDF 2.0 (V=5) encrypted PDFs.
    /// For earlier versions, use `authenticate_with_password` instead.
    ///
    /// # Arguments
    /// * `password` - The password to check (as UTF-8 bytes)
    ///
    /// # Returns
    /// `true` if the password is correct and the encryption key was derived
    pub fn check_owner_password(&mut self, password: &[u8]) -> bool {
        if let Some(ref mut encrypt_dict) = self.encrypt_dict {
            encrypt_dict.check_owner_password(password)
        } else {
            false // Not encrypted
        }
    }

    /// Authenticates with a password and derives the encryption key.
    ///
    /// This works for all PDF versions. For V=1,2,4 it requires the file ID,
    /// which is obtained from the trailer.
    ///
    /// # Arguments
    /// * `password` - The password to check (as UTF-8 bytes)
    ///
    /// # Returns
    /// `true` if the password is correct and the encryption key was derived
    ///
    /// # Note
    /// For PDF 2.0 (V=5), the file ID is not required. For earlier versions,
    /// the file ID must have been stored in the trailer during parsing.
    pub fn authenticate_with_password(&mut self, password: &[u8]) -> PDFResult<bool> {
        // Pull the data we need from XRef up front so `self.encrypt_dict` can be
        // mutably borrowed below without overlapping an immutable `self.xref()` borrow.
        let file_id = self.xref().file_id()?;
        let encrypt_obj_num = self.xref().get_encrypt_dict_ref().and_then(|r| match r {
            PDFObject::Ref(r) => Some(r.num),
            _ => None,
        });

        if let Some(ref mut encrypt_dict) = self.encrypt_dict {
            let keyed = if matches!(
                encrypt_dict.encryption_version(),
                EncryptionVersion::V5R5 | EncryptionVersion::V5R6
            ) {
                encrypt_dict.check_user_password(password)
            } else {
                // Legacy (V=1/2/4) needs the file ID from the trailer.
                encrypt_dict.derive_encryption_key_with_file_id(password, &file_id)
            };
            if keyed {
                // Install the decryptor so subsequent fetches decrypt streams/strings inline.
                // Clone the encrypt dict before the mutable `xref_mut` borrow so the
                // two mutable borrows of `self` don't overlap.
                let encrypt_clone = encrypt_dict.clone();
                self.xref_mut()
                    .set_decryptor(Some(encrypt_clone), encrypt_obj_num);
            }
            Ok(keyed)
        } else {
            Ok(false) // Not encrypted
        }
    }

    /// Gets the file encryption key (if password has been authenticated).
    ///
    /// # Returns
    /// The encryption key, or an error if no password has been authenticated
    pub fn get_encryption_key(&self) -> PDFResult<&[u8]> {
        self.encrypt_dict
            .as_ref()
            .and_then(|e| e.encryption_key.as_deref())
            .ok_or_else(|| PDFError::parse_error("No encryption key - authenticate first", None))
    }

    /// Gets the first page of a linearized PDF with progressive loading optimization.
    ///
    /// For linearized PDFs, this method can load the first page without needing
    /// to load the entire file. This is useful for web viewers that want to display
    /// the first page quickly.
    ///
    /// # Returns
    /// `Some(Page)` if the PDF is linearized and first page can be loaded, `None` otherwise
    ///
    /// # Note
    /// This is a simplified implementation. A full implementation would use the
    /// hint table to load shared resources incrementally.
    pub fn get_first_page_linearized(&mut self) -> PDFResult<Option<Page>> {
        let first_page_obj_num = match &self.linearized {
            Some(info) => info.first_page_obj_num,
            None => return Ok(None),
        };

        // For linearized PDFs, try to fetch the first page object directly
        // without parsing the entire page tree
        match self.doc_store.resolve_ref(first_page_obj_num, 0) {
            Ok(page_obj) => {
                // Create a page object with the first page index (0)
                // We don't have the reference, so use None for page_ref
                let page = Page::new(0, page_obj, None);
                Ok(Some(page))
            }
            Err(_) => {
                // Fall back to regular page loading
                self.get_page(0).map(Some)
            }
        }
    }

    /// Gets the PDF version from the document header.
    ///
    /// PDF version is specified in the header as "%PDF-1.x" at the start of the file.
    ///
    /// # Returns
    /// The PDF version as a string (e.g., "1.4", "1.7"), or an error if not found.
    pub fn pdf_version(&mut self) -> PDFResult<String> {
        // Cursor-free range read (D.3): no stream position to save/restore.
        // Read first 10 bytes to find header
        let header_bytes = self.xref_mut().get_bytes(0, 10)?;

        // Check for "%PDF-" header
        let header_str = String::from_utf8_lossy(&header_bytes);
        if let Some(version_pos) = header_str.find("%PDF-") {
            let version_start = version_pos + 5; // Skip "%PDF-"
            // Get up to 3 more characters for version (e.g., "1.4")
            let version_end = (version_start + 3).min(header_str.len());
            let version = header_str[version_start..version_end].trim();
            return Ok(version.to_string());
        }

        Err(PDFError::Generic(
            "PDF version not found in header".to_string(),
        ))
    }

    /// Gets the document info dictionary.
    ///
    /// The document info dictionary contains metadata like Title, Author, Creator, etc.
    ///
    /// # Returns
    /// `Some(PDFObject)` with the info dictionary, or `None` if not present.
    pub fn document_info(&mut self) -> PDFResult<Option<Arc<PDFObject>>> {
        // Check delta trailer overrides first (e.g. after set_info).
        let info_ref = self
            .doc_store
            .delta()
            .get_trailer_override("Info")
            .cloned()
            .or_else(|| {
                let trailer = self.xref().trailer();
                let trailer_dict = match trailer {
                    Some(PDFObject::Dictionary(dict)) => dict,
                    _ => return None,
                };
                trailer_dict.get("Info").cloned()
            });
        let info_ref = match info_ref {
            Some(r) => r,
            None => return Ok(None),
        };
        let info_obj = self.doc_store.resolve_if_ref(&info_ref)?;
        Ok(Some(info_obj))
    }

    /// Gets the document outline (bookmarks) dictionary.
    ///
    /// The outline contains hierarchical bookmarks that point to destinations in the PDF.
    ///
    /// # Returns
    /// `Some(PDFObject)` with the outlines dictionary, or `None` if not present.
    pub fn document_outline(&mut self) -> PDFResult<Option<Arc<PDFObject>>> {
        let outlines_ref = {
            let catalog = self.catalog();

            let catalog = match catalog {
                Some(c) => c,
                None => return Ok(None),
            };

            if let PDFObject::Dictionary(cat_dict) = catalog {
                match cat_dict.get("Outlines") {
                    Some(o) => o.clone(),
                    None => return Ok(None),
                }
            } else {
                return Ok(None);
            }
        };

        let outlines = self.doc_store.resolve_if_ref(&outlines_ref)?;
        Ok(Some(outlines))
    }

    /// Gets the document outline items (bookmarks) as a structured tree.
    ///
    /// This is a convenience method that returns parsed outline items
    /// instead of just the raw outline dictionary.
    ///
    /// # Returns
    /// `Some(Vec<OutlineItem>)` with hierarchical bookmark structure, or `None` if not present.
    pub fn document_outline_items(
        &mut self,
    ) -> PDFResult<Option<Vec<crate::core::outline::OutlineItem>>> {
        crate::core::outline::parse_document_outline(self)
    }

    /// Gets the named destinations dictionary from the document catalog.
    ///
    /// Named destinations are bookmarks that can be referenced by name from
    /// outlines, links, and other actions.
    ///
    /// # Returns
    /// `Some(PDFObject)` with the destinations dictionary, or `None` if not present.
    pub fn document_dests(&mut self) -> PDFResult<Option<Arc<PDFObject>>> {
        let catalog = match self.catalog().cloned() {
            Some(c) => c,
            None => return Ok(None),
        };

        if let PDFObject::Dictionary(cat_dict) = catalog {
            match cat_dict.get("Dests") {
                Some(dests_ref) => {
                    let dests = self.doc_store.resolve_if_ref(dests_ref)?;
                    Ok(Some(dests))
                }
                None => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Resolves a named destination to a page index and destination type.
    ///
    /// This method looks up a named destination in the /Dests dictionary and
    /// resolves it to a page index with optional destination parameters.
    ///
    /// # Arguments
    /// * `name` - The destination name (as a string)
    ///
    /// # Returns
    /// * `Some((page_index, dest_type))` - The page index and destination type
    /// * `None` - If the named destination is not found
    pub fn resolve_named_destination(
        &mut self,
        name: &str,
    ) -> PDFResult<Option<(usize, crate::core::outline::DestinationType)>> {
        let dests = match self.document_dests()? {
            Some(d) => d,
            None => return Ok(None),
        };

        // /Dests can be a dictionary or a Name tree
        // For now, we only support the dictionary format
        if let PDFObject::Dictionary(dests_dict) = &*dests {
            // Look up the destination by name
            // Names in /Dests are stored as Name objects or strings
            let dest_obj = dests_dict.get(name).or_else(|| {
                // Try as a Name object (with / prefix)
                dests_dict.get(&format!("/{}", name))
            });

            match dest_obj {
                Some(dest) => {
                    // Resolve the destination
                    let dest = self.doc_store.resolve_if_ref(dest)?;

                    // Destinations can be:
                    // 1. An array: [page_ref, /Type, params...]
                    // 2. A string: name of another destination (not supported yet)
                    // 3. A reference to another destination

                    match &*dest {
                        PDFObject::Array(arr) => {
                            if arr.is_empty() {
                                return Ok(None);
                            }

                            // First element is the page reference
                            let page_ref = &arr[0];

                            // Resolve page reference to page index
                            let page_index = match &**page_ref {
                                PDFObject::Ref(ref_obj) => {
                                    match self.resolve_page_index(ref_obj.num, ref_obj.generation) {
                                        Some(idx) => idx,
                                        None => return Ok(None),
                                    }
                                }
                                _ => return Ok(None),
                            };

                            // Second element is the destination type name
                            let dest_type = if arr.len() > 1 {
                                match &*arr[1] {
                                    PDFObject::Name(type_name) => {
                                        crate::core::outline::parse_destination_type(
                                            type_name,
                                            &arr[2..],
                                        )?
                                    }
                                    _ => crate::core::outline::DestinationType::Fit,
                                }
                            } else {
                                crate::core::outline::DestinationType::Fit
                            };

                            Ok(Some((page_index, dest_type)))
                        }
                        PDFObject::Ref(ref_obj) => {
                            // Fetch the referenced destination
                            let resolved_dest = self
                                .doc_store
                                .resolve_ref(ref_obj.num, ref_obj.generation)?;
                            match &*resolved_dest {
                                PDFObject::Array(arr) => {
                                    if arr.is_empty() {
                                        return Ok(None);
                                    }

                                    let page_ref = &arr[0];
                                    let page_index = match &**page_ref {
                                        PDFObject::Ref(ref_obj) => {
                                            match self
                                                .resolve_page_index(ref_obj.num, ref_obj.generation)
                                            {
                                                Some(idx) => idx,
                                                None => return Ok(None),
                                            }
                                        }
                                        _ => return Ok(None),
                                    };

                                    let dest_type = if arr.len() > 1 {
                                        match &*arr[1] {
                                            PDFObject::Name(type_name) => {
                                                crate::core::outline::parse_destination_type(
                                                    type_name,
                                                    &arr[2..],
                                                )?
                                            }
                                            _ => crate::core::outline::DestinationType::Fit,
                                        }
                                    } else {
                                        crate::core::outline::DestinationType::Fit
                                    };

                                    Ok(Some((page_index, dest_type)))
                                }
                                _ => Ok(None),
                            }
                        }
                        _ => Ok(None),
                    }
                }
                None => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Gets the page labels dictionary from the document catalog.
    ///
    /// Page labels allow custom page numbering (e.g., Roman numerals for front matter).
    ///
    /// # Returns
    /// `Some(PDFObject)` with the page labels number tree, or `None` if not present.
    pub fn page_labels(&mut self) -> PDFResult<Option<Arc<PDFObject>>> {
        let catalog = match self.catalog().cloned() {
            Some(c) => c,
            None => return Ok(None),
        };

        if let PDFObject::Dictionary(cat_dict) = catalog {
            match cat_dict.get("PageLabels") {
                Some(labels_ref) => {
                    let labels = self.doc_store.resolve_if_ref(labels_ref)?;
                    Ok(Some(labels))
                }
                None => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Gets the page label for a specific page index.
    ///
    /// Page labels allow custom page numbering like "i", "ii", "iii" for front matter,
    /// or "A-1", "A-2" for appendix pages.
    ///
    /// # Arguments
    /// * `page_index` - The zero-based page index
    ///
    /// # Returns
    /// The page label as a string, or the default page number (1-indexed) if no custom label
    pub fn get_page_label(&mut self, page_index: usize) -> PDFResult<String> {
        // Default to 1-indexed page number
        let default_label = (page_index + 1).to_string();

        let labels_obj = match self.page_labels()? {
            Some(l) => l,
            None => return Ok(default_label),
        };

        // PageLabels can be a number tree (dictionary with /Kids or /Nums)
        // For now, we'll handle the simple dictionary case with /Nums array
        if let PDFObject::Dictionary(labels_dict) = &*labels_obj {
            if let Some(nums_obj) = labels_dict.get("Nums") {
                let nums = match &*self.doc_store.resolve_if_ref(nums_obj)? {
                    PDFObject::Array(arr) => arr.clone(),
                    _ => return Ok(default_label),
                };

                // /Nums is an array of [start_page_1, label_dict_1, start_page_2, label_dict_2, ...]
                // Find the label range that contains our page_index
                let mut current_label = None;

                let mut i = 0;
                while i < nums.len() {
                    if i + 1 >= nums.len() {
                        break;
                    }

                    let start_page = match nums[i].as_ref() {
                        PDFObject::Number(n) => *n as usize,
                        _ => {
                            i += 2;
                            continue;
                        }
                    };

                    if start_page > page_index {
                        // We've passed our page index, use the previous label
                        break;
                    }

                    let label_dict = match nums[i + 1].as_ref() {
                        PDFObject::Dictionary(dict) => {
                            // Use a reference to the dictionary
                            dict
                        }
                        PDFObject::Ref(ref_obj) => {
                            match self.doc_store.resolve_ref(ref_obj.num, ref_obj.generation) {
                                Ok(obj) => match &*obj {
                                    PDFObject::Dictionary(_) => {
                                        // We need to convert from &HashMap<_, _, RandomState> to use
                                        // For now, skip this complex case
                                        i += 2;
                                        continue;
                                    }
                                    _ => {
                                        i += 2;
                                        continue;
                                    }
                                },
                                Err(_) => {
                                    i += 2;
                                    continue;
                                }
                            }
                        }
                        _ => {
                            i += 2;
                            continue;
                        }
                    };

                    // Parse the label
                    let prefix = match label_dict.get("P") {
                        Some(PDFObject::String(bytes)) => {
                            String::from_utf8_lossy(bytes).to_string()
                        }
                        Some(PDFObject::HexString(bytes)) => {
                            String::from_utf8_lossy(bytes).to_string()
                        }
                        _ => String::new(),
                    };

                    let start_from = match label_dict.get("St") {
                        Some(PDFObject::Number(n)) => *n as usize,
                        _ => 1,
                    };

                    let numbering_style = match label_dict.get("S") {
                        Some(PDFObject::Name(name)) => name.as_str(),
                        _ => "D",
                    };

                    let page_number = start_from + (page_index - start_page);
                    let number_label = match numbering_style {
                        "D" => page_number.to_string(),
                        "R" => to_roman_numeral(page_number),
                        "r" => to_roman_numeral(page_number).to_lowercase(),
                        "A" => to_alpha_label(page_number - 1),
                        "a" => to_alpha_label(page_number - 1).to_lowercase(),
                        _ => page_number.to_string(),
                    };

                    current_label = Some(format!("{}{}", prefix, number_label));
                    i += 2;
                }

                if let Some(label) = current_label {
                    return Ok(label);
                }
            }
        }

        Ok(default_label)
    }
}

/// Converts a number to a Roman numeral.
fn to_roman_numeral(mut num: usize) -> String {
    if num == 0 {
        return String::new();
    }
    if num > 3999 {
        return num.to_string(); // Too large for Roman numerals
    }

    let values = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];

    let mut result = String::new();

    for (value, name) in values.iter() {
        while num >= *value {
            result.push_str(name);
            num -= *value;
        }
        if num == 0 {
            break;
        }
    }

    result
}

/// Converts a number to an alphabetic label (A, B, ..., Z, AA, AB, ...).
fn to_alpha_label(mut num: usize) -> String {
    if num == 0 {
        return "A".to_string();
    }

    let mut result = Vec::new();

    while num > 0 {
        num -= 1; // Convert to 0-indexed
        result.push(((b'A' as usize) + (num % 26)) as u8 as char);
        num /= 26;
    }

    result.reverse();
    result.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a minimal valid PDF document for testing.
    fn create_minimal_pdf() -> Vec<u8> {
        // A minimal PDF with:
        // - Catalog (object 1)
        // - Pages dict (object 2)
        // - Single page (object 3)
        // - xref table
        // - trailer
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000115 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            162\n\
            %%EOF\n";

        pdf.to_vec()
    }

    #[test]
    fn test_find_startxref() {
        let pdf = create_minimal_pdf();
        let offset = PDFDocument::find_startxref(&pdf).unwrap();
        assert_eq!(offset, 162);
    }

    #[test]
    fn test_open_minimal_pdf() {
        let pdf = create_minimal_pdf();
        let doc = PDFDocument::open(pdf).unwrap();

        assert!(doc.catalog().is_some());

        let catalog = doc.catalog().unwrap();
        match catalog {
            PDFObject::Dictionary(dict) => {
                assert!(dict.contains_key("Type"));
                assert!(dict.contains_key("Pages"));
            }
            _ => panic!("Expected catalog to be a dictionary"),
        }
    }

    #[test]
    fn test_get_pages_dict() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        let pages = doc.pages_dict().unwrap();

        match &*pages {
            PDFObject::Dictionary(dict) => {
                assert_eq!(
                    dict.get("Type"),
                    Some(&PDFObject::Name("Pages".to_string()))
                );
                assert!(dict.contains_key("Kids"));
                assert!(dict.contains_key("Count"));
            }
            _ => panic!("Expected pages to be a dictionary"),
        }
    }

    #[test]
    fn test_page_count() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        let count = doc.page_count().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_page_simple() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Get the first (and only) page
        let page = doc.get_page(0).unwrap();
        assert_eq!(page.index(), 0);

        // Verify it's a page dictionary
        match page.dict() {
            PDFObject::Dictionary(dict) => {
                assert_eq!(dict.get("Type"), Some(&PDFObject::Name("Page".to_string())));
                assert!(dict.contains_key("Parent"));
            }
            _ => panic!("Expected page dict to be a dictionary"),
        }
    }

    #[test]
    fn test_get_page_multiple_pages() {
        // Create a PDF with 3 pages (flat structure)
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            xref\n\
            0 6\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000127 00000 n\n\
            0000000198 00000 n\n\
            0000000269 00000 n\n\
            trailer\n\
            << /Size 6 /Root 1 0 R >>\n\
            startxref\n\
            340\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Verify page count
        assert_eq!(doc.page_count().unwrap(), 3);

        // Get each page and verify index
        let page0 = doc.get_page(0).unwrap();
        assert_eq!(page0.index(), 0);

        let page1 = doc.get_page(1).unwrap();
        assert_eq!(page1.index(), 1);

        let page2 = doc.get_page(2).unwrap();
        assert_eq!(page2.index(), 2);
    }

    #[test]
    fn test_page_caching() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Get the page twice
        let page1 = doc.get_page(0).unwrap();
        let page2 = doc.get_page(0).unwrap();

        // Both should have the same index (verifies caching works)
        assert_eq!(page1.index(), page2.index());
        assert_eq!(page1.index(), 0);
    }

    #[test]
    fn test_get_page_out_of_bounds() {
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Try to get a page that doesn't exist
        let result = doc.get_page(99);
        assert!(result.is_err());
    }

    #[test]
    #[ignore] // TODO: Fix test PDF structure
    fn test_linearized_pdf_detection() {
        // Create a minimal linearized PDF
        let pdf = b"%PDF-1.4
1 0 obj
<< /Linearized 1.0 /L 1000 /H [ 10 5 ] /O 25 /P 3 /N 3 /T 500 >>
endobj

25 0 obj
<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] /Contents 4 0 R >>
endobj

2 0 obj
<< /Type /Pages /Kids [25 0 R 26 0 R 27 0 R] /Count 3 >>
endobj

3 0 obj
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>
endobj

4 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Hello World) Tj
ET
endstream
endobj

26 0 obj
<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] /Contents 5 0 R >>
endobj

5 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Page 2) Tj
ET
endstream
endobj

27 0 obj
<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R >> >> /MediaBox [0 0 612 792] /Contents 6 0 R >>
endobj

6 0 obj
<< /Length 44 >>
stream
BT
/F1 12 Tf
100 700 Td
(Page 3) Tj
ET
endstream
endobj

7 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj

xref
0 8
0000000000 65535 f
0000000010 00000 n
0000000480 00000 n
0000000460 00000 n
0000000360 00000 n
0000000560 00000 n
0000000540 00000 n
0000000640 00000 n
trailer
<< /Size 8 /Root 7 0 R >>
startxref
650
%%EOF";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Check that it's detected as linearized
        assert!(doc.is_linearized());

        // Check linearized info
        let info = doc.linearized_info().unwrap();
        assert_eq!(info.file_size, 1000);
        assert_eq!(info.first_page_obj_num, 3);
        assert_eq!(info.page_count, 3);
        assert_eq!(info.first_page_offset, 25);
        assert_eq!(info.primary_hint_offset, 10);
        assert_eq!(info.primary_hint_length, 5);
    }

    #[test]
    fn test_non_linearized_pdf() {
        // Use the existing minimal PDF test (which is not linearized)
        let pdf = create_minimal_pdf();
        let mut doc = PDFDocument::open(pdf).unwrap();

        // Should not be detected as linearized
        assert!(!doc.is_linearized());
        assert!(doc.linearized_info().is_none());
    }

    #[test]
    fn test_hierarchical_page_tree() {
        // Create a PDF with 2-level page tree:
        // Root Pages -> 2 intermediate Pages nodes -> 2 pages each
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R 4 0 R] /Count 4 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Pages /Kids [5 0 R 6 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Pages /Kids [7 0 R 8 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            6 0 obj\n\
            << /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            7 0 obj\n\
            << /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            8 0 obj\n\
            << /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            xref\n\
            0 9\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000121 00000 n\n\
            0000000198 00000 n\n\
            0000000275 00000 n\n\
            0000000346 00000 n\n\
            0000000417 00000 n\n\
            0000000488 00000 n\n\
            trailer\n\
            << /Size 9 /Root 1 0 R >>\n\
            startxref\n\
            559\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Verify page count
        assert_eq!(doc.page_count().unwrap(), 4);

        // Get all 4 pages and verify they have correct indices
        for i in 0..4 {
            let page = doc.get_page(i).unwrap();
            assert_eq!(page.index(), i);

            // Verify it's a page
            match page.dict() {
                PDFObject::Dictionary(dict) => {
                    assert_eq!(dict.get("Type"), Some(&PDFObject::Name("Page".to_string())));
                }
                _ => panic!("Expected page dict to be a dictionary"),
            }
        }
    }

    #[test]
    fn test_inherited_media_box() {
        // Create a PDF where MediaBox is defined at the Pages level, not on individual pages
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000139 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            186\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();
        let page = doc.get_page(0).unwrap();

        // Page itself doesn't have MediaBox
        assert!(page.get("MediaBox").is_none());

        // But we can get it via inheritance
        let media_box = doc.get_media_box(&page).unwrap();
        match &*media_box {
            PDFObject::Array(arr) => {
                assert_eq!(arr.len(), 4);
                assert_eq!(*arr[0], PDFObject::Number(0.0));
                assert_eq!(*arr[1], PDFObject::Number(0.0));
                assert_eq!(*arr[2], PDFObject::Number(612.0));
                assert_eq!(*arr[3], PDFObject::Number(792.0));
            }
            _ => panic!("Expected MediaBox to be an array"),
        }
    }

    #[test]
    fn test_inherited_resources() {
        // Create a PDF where Resources is defined at the Pages level
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /Font << >> >> >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000144 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            191\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();
        let page = doc.get_page(0).unwrap();

        // Get Resources via inheritance
        let resources = doc.get_resources(&page).unwrap();
        match &*resources {
            PDFObject::Dictionary(dict) => {
                assert!(dict.contains_key("Font"));
            }
            _ => panic!("Expected Resources to be a dictionary"),
        }
    }

    #[test]
    fn test_property_override() {
        // Create a PDF where MediaBox is defined at both Pages and Page level
        // Page level should take precedence
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>\n\
            endobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000139 00000 n\n\
            trailer\n\
            << /Size 4 /Root 1 0 R >>\n\
            startxref\n\
            210\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();
        let page = doc.get_page(0).unwrap();

        // Should get the page-level MediaBox (300x400), not the inherited one (612x792)
        let media_box = doc.get_media_box(&page).unwrap();
        match &*media_box {
            PDFObject::Array(arr) => {
                assert_eq!(*arr[2], PDFObject::Number(300.0));
                assert_eq!(*arr[3], PDFObject::Number(400.0));
            }
            _ => panic!("Expected MediaBox to be an array"),
        }
    }

    #[test]
    fn test_multi_level_inheritance() {
        // Create a hierarchical page tree where MediaBox is at the root Pages level
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R] /Count 2 /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Page /Parent 3 0 R >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 3 0 R >>\n\
            endobj\n\
            xref\n\
            0 6\n\
            0000000000 65535 f\n\
            0000000009 00000 n\n\
            0000000058 00000 n\n\
            0000000139 00000 n\n\
            0000000216 00000 n\n\
            0000000263 00000 n\n\
            trailer\n\
            << /Size 6 /Root 1 0 R >>\n\
            startxref\n\
            310\n\
            %%EOF\n";

        let mut doc = PDFDocument::open(pdf.to_vec()).unwrap();

        // Both pages should inherit MediaBox from the root Pages level
        for i in 0..2 {
            let page = doc.get_page(i).unwrap();
            let media_box = doc.get_media_box(&page).unwrap();
            match &*media_box {
                PDFObject::Array(arr) => {
                    assert_eq!(*arr[2], PDFObject::Number(612.0));
                    assert_eq!(*arr[3], PDFObject::Number(792.0));
                }
                _ => panic!("Expected MediaBox to be an array"),
            }
        }
    }

    #[test]
    fn highlight_undo_redo_and_incremental_reopen() {
        let original = create_minimal_pdf();
        let mut doc = PDFDocument::open(original.clone()).expect("minimal PDF should open");

        doc.add_highlight(0, [10.0, 20.0, 110.0, 40.0], [1.0, 0.9, 0.1])
            .expect("highlight should be added");
        assert_eq!(doc.editor_state(), (true, true, false));
        assert!(doc.get_page(0).unwrap().get("Annots").is_some());

        doc.undo().expect("highlight should undo");
        assert!(doc.get_page(0).unwrap().get("Annots").is_none());
        doc.redo().expect("highlight should redo");
        assert!(doc.get_page(0).unwrap().get("Annots").is_some());

        let update = doc
            .incremental_update(&original)
            .expect("update should serialize");
        let mut saved = original;
        saved.extend_from_slice(&update);
        let mut reopened = PDFDocument::open(saved).expect("incrementally saved PDF should reopen");
        let page = reopened.get_page(0).expect("saved page should resolve");
        let PDFObject::Array(annots) = page.get("Annots").expect("saved page should have Annots")
        else {
            panic!("Annots must be an array");
        };
        assert_eq!(annots.len(), 1);
    }

    #[test]
    fn rotate_page_undo_redo_and_incremental_reopen() {
        let original = create_minimal_pdf();
        let mut doc = PDFDocument::open(original.clone()).expect("minimal PDF should open");

        let page_before = doc.get_page(0).unwrap();
        assert!(page_before.get("Rotate").is_none());

        doc.rotate_page(0, 90).expect("rotate should succeed");
        assert_eq!(doc.editor_state(), (true, true, false));
        let page_90 = doc.get_page(0).unwrap();
        match page_90.get("Rotate").unwrap() {
            PDFObject::Number(n) => assert_eq!(*n, 90.0),
            other => panic!("expected Rotate=90, got {other:?}"),
        }

        doc.undo().expect("rotate undo should succeed");
        assert!(doc.get_page(0).unwrap().get("Rotate").is_none());

        doc.redo().expect("rotate redo should succeed");
        match doc.get_page(0).unwrap().get("Rotate").unwrap() {
            PDFObject::Number(n) => assert_eq!(*n, 90.0),
            other => panic!("expected Rotate=90 after redo, got {other:?}"),
        }

        let update = doc
                .incremental_update(&original)
                .expect("update should serialize");
        let mut saved = original;
        saved.extend_from_slice(&update);
        let mut reopened = PDFDocument::open(saved).expect("saved PDF should reopen");
        let page_reopened = reopened.get_page(0).expect("page should resolve");
        match page_reopened.get("Rotate").unwrap() {
            PDFObject::Number(n) => assert_eq!(*n, 90.0),
            other => panic!("expected Rotate=90 after reopen, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "multiple of 90")]
    fn rotate_page_rejects_invalid_angle() {
        let original = create_minimal_pdf();
        let mut doc = PDFDocument::open(original).expect("minimal PDF should open");
        let _ = doc.rotate_page(0, 45);
    }

    #[test]
    fn set_info_creates_and_round_trips() {
        let original = create_minimal_pdf();
        assert!(PDFDocument::open(original.clone())
            .unwrap()
            .document_info()
            .unwrap()
            .is_none());

        let mut doc = PDFDocument::open(original.clone()).expect("open");
        let title_val = SetInfoCommand::encode_info_string("Test Title");
        let author_val = SetInfoCommand::encode_info_string("pdf-x");
        doc.set_info(vec![
            ("Title".into(), Some(title_val)),
            ("Author".into(), Some(author_val)),
        ])
        .expect("set_info should succeed");
        assert_eq!(doc.editor_state(), (true, true, false));

        let info = doc.document_info().expect("info should resolve").unwrap();
        match info.as_ref() {
            PDFObject::Dictionary(dict) => {
                match &dict["Title"] {
                    PDFObject::String(s) => assert_eq!(s.as_slice(), b"Test Title"),
                    other => panic!("Title unexpected: {other:?}"),
                }
                match &dict["Author"] {
                    PDFObject::String(s) => assert_eq!(s.as_slice(), b"pdf-x"),
                    other => panic!("Author unexpected: {other:?}"),
                }
            }
            _ => panic!("info must be a dict"),
        }

        doc.undo().expect("undo");
        assert!(doc.document_info().unwrap().is_none());

        doc.redo().expect("redo");
        let info2 = doc.document_info().unwrap().unwrap();
        match info2.as_ref() {
            PDFObject::Dictionary(d) => assert_eq!(d.keys().len(), 2),
            _ => panic!("info must be a dict after redo"),
        }

        let update = doc
                .incremental_update(&original)
                .expect("serialize");
        let mut saved = original;
        saved.extend_from_slice(&update);
        let mut reopened = PDFDocument::open(saved).expect("reopen");
        let info3 = reopened.document_info().expect("info").unwrap();
        match info3.as_ref() {
            PDFObject::Dictionary(d) => {
                match &d["Title"] {
                    PDFObject::String(s) => assert_eq!(s.as_slice(), b"Test Title"),
                    other => panic!("Title after reopen: {other:?}"),
                }
            }
            _ => panic!("info must be a dict after reopen"),
        }
    }

    #[test]
    fn set_info_non_ascii_uses_utf16be() {
        let original = create_minimal_pdf();
        let mut doc = PDFDocument::open(original).expect("open");
        let val = SetInfoCommand::encode_info_string("中文标题");
        doc.set_info(vec![("Title".into(), Some(val))]).expect("set_info");
        let info = doc.document_info().unwrap().unwrap();
        // document_info returns the whole /Info dict.
        let PDFObject::Dictionary(d) = info.as_ref() else {
            panic!("expected dict, got {info:?}");
        };
        let PDFObject::HexString(bytes) = &d["Title"] else {
            panic!("Title should be HexString, got {:?}", &d["Title"]);
        };
        assert_eq!(bytes[0], 0xFE);
        assert_eq!(bytes[1], 0xFF);
        // 4 CJK chars x 2 bytes + 2 BOM = 10.
        assert_eq!(bytes.len(), 10);
    }

    fn create_3page_flat_pdf() -> Vec<u8> {
        // Flat page tree: root /Pages -> 3 page kids.
        // Objects: 1=Catalog, 2=Pages, 3-5=Pages, 6=Font, 7-9=Contents streams.
        b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> >> /Contents 7 0 R >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> >> /Contents 8 0 R >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> >> /Contents 9 0 R >>\n\
            endobj\n\
            6 0 obj\n\
            << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\n\
            endobj\n\
            7 0 obj\n\
            << /Length 21 >>\n\
            stream\n\
            BT /F1 12 Tf 100 700 Td (P1) Tj ET\n\
            endstream\n\
            endobj\n\
            8 0 obj\n\
            << /Length 21 >>\n\
            stream\n\
            BT /F1 12 Tf 100 700 Td (P2) Tj ET\n\
            endstream\n\
            endobj\n\
            9 0 obj\n\
            << /Length 21 >>\n\
            stream\n\
            BT /F1 12 Tf 100 700 Td (P3) Tj ET\n\
            endstream\n\
            endobj\n\
            xref\n\
            0 10\n\
            0000000000 65535 f \n\
            0000000009 00000 n \n\
            0000000058 00000 n \n\
            0000000121 00000 n \n\
            0000000272 00000 n \n\
            0000000423 00000 n \n\
            0000000574 00000 n \n\
            0000000651 00000 n \n\
            0000000740 00000 n \n\
            0000000829 00000 n \n\
            trailer\n\
            << /Size 10 /Root 1 0 R >>\n\
            startxref\n\
            918\n\
            %%EOF\n\n\
            "
            .to_vec()
    }

    fn create_nested_tree_pdf() -> Vec<u8> {
        // 2-level tree: root /Pages -> 2 intermediate nodes -> 2 pages each.
        // Objects: 1=Catalog, 2=RootPages, 3,4=IntermediatePages, 5-8=Page.
        b"%PDF-1.4\n\
            1 0 obj\n\
            << /Type /Catalog /Pages 2 0 R >>\n\
            endobj\n\
            2 0 obj\n\
            << /Type /Pages /Kids [3 0 R 4 0 R] /Count 4 >>\n\
            endobj\n\
            3 0 obj\n\
            << /Type /Pages /Kids [5 0 R 6 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            4 0 obj\n\
            << /Type /Pages /Kids [7 0 R 8 0 R] /Count 2 /Parent 2 0 R >>\n\
            endobj\n\
            5 0 obj\n\
            << /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            6 0 obj\n\
            << /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            7 0 obj\n\
            << /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            8 0 obj\n\
            << /Type /Page /Parent 4 0 R /MediaBox [0 0 612 792] >>\n\
            endobj\n\
            xref\n\
            0 9\n\
            0000000000 65535 f \n\
            0000000009 00000 n \n\
            0000000058 00000 n \n\
            0000000121 00000 n \n\
            0000000198 00000 n \n\
            0000000275 00000 n \n\
            0000000346 00000 n \n\
            0000000417 00000 n \n\
            0000000488 00000 n \n\
            trailer\n\
            << /Size 9 /Root 1 0 R >>\n\
            startxref\n\
            559\n\
            %%EOF\n\n\
            "
            .to_vec()
    }

    #[test]
    fn delete_page_flat_tree_first() {
        let original = create_3page_flat_pdf();
        let mut doc = PDFDocument::open(original.clone()).expect("open");
        assert_eq!(doc.page_count().unwrap(), 3);

        doc.delete_page(0).expect("delete");
        assert_eq!(doc.page_count().unwrap(), 2);
        assert_eq!(doc.editor_state(), (true, true, false));

        // Page 1 (was page 2) should now be at index 0.
        let p = doc.get_page(0).unwrap();
        assert_eq!(p.index(), 0);

        // Page 2 (was page 3) should now be at index 1.
        let p = doc.get_page(1).unwrap();
        assert_eq!(p.index(), 1);

        // Out of bounds.
        assert!(doc.get_page(2).is_err());

        // Undo restores.
        doc.undo().expect("undo");
        assert_eq!(doc.page_count().unwrap(), 3);
        doc.get_page(2).expect("page 2 restored");

        // Redo.
        doc.redo().expect("redo");
        assert_eq!(doc.page_count().unwrap(), 2);
    }

    #[test]
    fn delete_page_flat_tree_middle() {
        let original = create_3page_flat_pdf();
        let mut doc = PDFDocument::open(original).expect("open");
        doc.delete_page(1).expect("delete");
        assert_eq!(doc.page_count().unwrap(), 2);

        // Page 0 still accessible.
        doc.get_page(0).expect("page 0");
        // Page 1 (was page 2) accessible.
        doc.get_page(1).expect("page 1");
        assert!(doc.get_page(2).is_err());
    }

    #[test]
    fn delete_page_flat_tree_last() {
        let original = create_3page_flat_pdf();
        let mut doc = PDFDocument::open(original).expect("open");
        doc.delete_page(2).expect("delete");
        assert_eq!(doc.page_count().unwrap(), 2);
        doc.get_page(0).expect("page 0");
        doc.get_page(1).expect("page 1");
    }

    #[test]
    fn delete_page_nested_tree() {
        let original = create_nested_tree_pdf();
        let mut doc = PDFDocument::open(original.clone()).expect("open");
        assert_eq!(doc.page_count().unwrap(), 4);

        // Delete page 1 (second page, in first intermediate node).
        doc.delete_page(1).expect("delete");
        assert_eq!(doc.page_count().unwrap(), 3);

        // Remaining pages accessible.
        for i in 0..3 {
            doc.get_page(i).unwrap_or_else(|_| {
                panic!("page {i} should exist after delete");
            });
        }
        assert!(doc.get_page(3).is_err());

        // Undo.
        doc.undo().expect("undo");
        assert_eq!(doc.page_count().unwrap(), 4);

        // Redo.
        doc.redo().expect("redo");
        assert_eq!(doc.page_count().unwrap(), 3);

        // Round-trip: save and reopen.
        let update = doc
            .incremental_update(&original)
            .expect("update should serialize");
        let mut saved = original;
        saved.extend_from_slice(&update);
        let mut reopened =
            PDFDocument::open(saved).expect("saved PDF should reopen");
        assert_eq!(reopened.page_count().unwrap(), 3);
    }

    #[test]
    fn delete_page_nested_tree_cross_branch() {
        let original = create_nested_tree_pdf();
        let mut doc = PDFDocument::open(original).expect("open");

        // Delete page 2 (first page of second intermediate node).
        // This tests that /Count is decremented on both the intermediate
        // node (4) and the root (2).
        doc.delete_page(2).expect("delete");
        assert_eq!(doc.page_count().unwrap(), 3);
    }
}
