//! Delta layer for tracking PDF document modifications.
//!
//! The delta layer enables editing capabilities while keeping the base PDF immutable.
//! All modifications are tracked separately and can be applied as incremental updates.

use crate::core::error::{PDFError, PDFResult};
use crate::core::parser::{PDFObject, Ref};
use std::collections::{HashMap, HashSet};

/// Object reference ID (object number and generation number).
pub type ObjectId = (u32, u32);

/// Modification delta tracking all changes to a PDF document.
///
/// The delta layer maintains three separate collections:
/// - **Modified objects**: Overrides for objects in the base PDF
/// - **New objects**: Objects that don't exist in the base PDF
/// - **Deleted objects**: Objects marked for removal
///
/// This design preserves progressive loading - the base PDF remains
/// completely unchanged and immutable.
pub struct DeltaLayer {
    /// Modified objects (overwrites base PDF objects)
    /// Key: (object_num, generation), Value: object data
    modified: HashMap<ObjectId, DeltaObject>,

    /// Newly created objects (don't exist in base PDF)
    /// Assigned new object numbers starting from base PDF size
    new_objects: Vec<DeltaObject>,

    /// Deletion markers (objects marked as deleted)
    /// Returns error when attempting to fetch these
    deleted: HashSet<ObjectId>,

    /// Next available object number for new objects
    next_obj_num: u32,

    /// Initial base PDF size (for clearing)
    base_pdf_size: u32,

    /// Trailer-level overrides (e.g. /Info → new ref).
    ///
    /// Merged into the trailer by [`PDFWriter::write_trailer`] during
    /// incremental save. This is the mechanism for commands that
    /// modify trailer entries (like setting document metadata).
    trailer_overrides: HashMap<String, PDFObject>,

    /// Monotonic document-state revision used to invalidate derived caches.
    revision: u64,
}

impl std::fmt::Debug for DeltaLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeltaLayer")
            .field("modified", &self.modified)
            .field("new_objects", &self.new_objects)
            .field("deleted", &self.deleted)
            .field("trailer_overrides", &self.trailer_overrides)
            .field("next_obj_num", &self.next_obj_num)
            .field("base_pdf_size", &self.base_pdf_size)
            .field("revision", &self.revision)
            .finish()
    }
}

/// Object in the delta layer with metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaObject {
    /// The actual PDF object
    pub object: PDFObject,

    /// Object number (assigned for new objects, or copied from base)
    pub obj_num: u32,

    /// Generation number (always 0 for new objects)
    pub generation: u32,
}

/// Callback type for fetching objects from the base PDF.
///
/// This function takes an object reference and returns the object
/// from the base PDF (or an error if not found).
///
/// Note: This is a function pointer rather than a trait object to allow
/// stack-allocated closures to be passed by reference without requiring `Rc`.
pub type BaseObjectFetcher<'a> = dyn Fn(Ref) -> PDFResult<PDFObject> + 'a;

/// Command for reversible operations (undo/redo).
///
/// All modifications through the delta layer should use commands
/// to enable undo/redo functionality.
pub trait Command: Send {
    /// Execute the command
    ///
    /// # Arguments
    /// * `delta` - The delta layer to modify
    /// * `fetch_base` - Optional callback to fetch objects from the base PDF
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()>;

    /// Undo the command
    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()>;

    /// Redo the command
    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()>;
}

/// Local undo/redo state for an editor session.
///
/// This is deliberately separate from [`DeltaLayer`]: the delta is the
/// serializable document state, while command history is ephemeral UI state.
#[derive(Default)]
pub struct CommandHistory {
    executed: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
}

impl std::fmt::Debug for CommandHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandHistory")
            .field("executed_count", &self.executed.len())
            .field("redo_count", &self.redo.len())
            .finish()
    }
}

impl CommandHistory {
    /// Create an empty local history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute a command against `delta` and record it after success.
    pub fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        mut command: Box<dyn Command>,
        fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        command.execute(delta, fetch_base)?;
        self.executed.push(command);
        self.redo.clear();
        Ok(())
    }

    /// Undo the most recently executed command.
    pub fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        let mut command = self
            .executed
            .pop()
            .ok_or_else(|| PDFError::Generic("Nothing to undo".into()))?;
        command.undo(delta)?;
        self.redo.push(command);
        Ok(())
    }

    /// Redo the most recently undone command.
    pub fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        let mut command = self
            .redo
            .pop()
            .ok_or_else(|| PDFError::Generic("Nothing to redo".into()))?;
        command.redo(delta)?;
        self.executed.push(command);
        Ok(())
    }

    /// Drop local undo/redo state without changing the document delta.
    pub fn clear(&mut self) {
        self.executed.clear();
        self.redo.clear();
    }

    /// Whether an undo operation is available.
    pub fn can_undo(&self) -> bool {
        !self.executed.is_empty()
    }

    /// Whether a redo operation is available.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

impl DeltaLayer {
    /// Create a new empty delta layer.
    ///
    /// # Arguments
    /// * `base_pdf_size` - Number of objects in the base PDF (used for assigning new object numbers)
    ///
    /// # Example
    /// ```
    /// use pdf_x_core::core::DeltaLayer;
    ///
    /// let delta = DeltaLayer::new(100); // Base PDF has 100 objects
    /// ```
    pub fn new(base_pdf_size: u32) -> Self {
        Self {
            modified: HashMap::new(),
            new_objects: Vec::new(),
            deleted: HashSet::new(),
            next_obj_num: base_pdf_size,
            base_pdf_size,
            trailer_overrides: HashMap::new(),
            revision: 0,
        }
    }

    /// Modify an existing object from the base PDF.
    ///
    /// This adds an override that will be returned instead of the base object
    /// when resolving object references.
    ///
    /// # Arguments
    /// * `obj_ref` - Reference to the object to modify
    /// * `new_obj` - New object data
    ///
    /// # Example
    /// ```
    /// use pdf_x_core::core::{DeltaLayer, parser::PDFObject, Ref};
    ///
    /// let mut delta = DeltaLayer::new(100);
    /// delta.modify_object(
    ///     Ref::new(5, 0),
    ///     PDFObject::Number(42.0)
    /// );
    /// ```
    pub fn modify_object(&mut self, obj_ref: Ref, new_obj: PDFObject) {
        let key = (obj_ref.num, obj_ref.generation);
        let replacement = DeltaObject {
            object: new_obj,
            obj_num: obj_ref.num,
            generation: obj_ref.generation,
        };

        let removed_deletion = self.deleted.remove(&key);
        let changed_object = self.modified.get(&key) != Some(&replacement);
        if changed_object {
            self.modified.insert(key, replacement);
        }
        if removed_deletion || changed_object {
            self.bump_revision();
        }
    }

    /// Add a new object (doesn't exist in base PDF).
    ///
    /// Returns the reference assigned to this new object.
    ///
    /// # Arguments
    /// * `obj` - The object to add
    ///
    /// # Returns
    /// Reference to the newly added object
    ///
    /// # Example
    /// ```
    /// use pdf_x_core::core::{DeltaLayer, parser::PDFObject};
    ///
    /// let mut delta = DeltaLayer::new(100);
    /// let ref_num = delta.add_object(PDFObject::Number(42.0));
    /// assert_eq!(ref_num.num, 100); // First new object gets number 100
    /// ```
    pub fn add_object(&mut self, obj: PDFObject) -> Ref {
        let obj_num = self.next_obj_num;
        self.next_obj_num += 1;

        let delta_obj = DeltaObject {
            object: obj,
            obj_num,
            generation: 0, // New objects always have generation 0
        };

        self.new_objects.push(delta_obj);
        self.bump_revision();

        Ref::new(obj_num, 0)
    }

    /// Restore an object at a previously allocated reference (used by redo).
    pub fn restore_new_object(&mut self, obj_ref: Ref, obj: PDFObject) {
        let mut changed = self.deleted.remove(&obj_ref.as_id());
        if let Some(existing) = self
            .new_objects
            .iter_mut()
            .find(|item| item.obj_num == obj_ref.num && item.generation == obj_ref.generation)
        {
            if existing.object != obj {
                existing.object = obj;
                changed = true;
            }
        } else {
            self.new_objects.push(DeltaObject {
                object: obj,
                obj_num: obj_ref.num,
                generation: obj_ref.generation,
            });
            changed = true;
        }

        let next_obj_num = self.next_obj_num.max(obj_ref.num.saturating_add(1));
        if next_obj_num != self.next_obj_num {
            self.next_obj_num = next_obj_num;
            changed = true;
        }
        if changed {
            self.bump_revision();
        }
    }

    /// Remove a newly-created object while retaining its object number for redo.
    pub fn remove_new_object(&mut self, obj_ref: Ref) {
        let previous_len = self.new_objects.len();
        self.new_objects
            .retain(|item| item.obj_num != obj_ref.num || item.generation != obj_ref.generation);
        let removed_deletion = self.deleted.remove(&obj_ref.as_id());
        if self.new_objects.len() != previous_len || removed_deletion {
            self.bump_revision();
        }
    }

    /// Delete an object (mark as deleted).
    ///
    /// Deleted objects will return an error when attempting to fetch them.
    ///
    /// # Arguments
    /// * `obj_ref` - Reference to the object to delete
    ///
    /// # Example
    /// ```
    /// use pdf_x_core::core::{DeltaLayer, Ref};
    ///
    /// let mut delta = DeltaLayer::new(100);
    /// delta.delete_object(Ref::new(5, 0));
    /// assert!(delta.is_deleted(&Ref::new(5, 0)));
    /// ```
    pub fn delete_object(&mut self, obj_ref: Ref) {
        let key = (obj_ref.num, obj_ref.generation);
        let inserted_deletion = self.deleted.insert(key);

        // Remove from modified if it was previously modified
        let removed_modification = self.modified.remove(&key).is_some();
        if inserted_deletion || removed_modification {
            self.bump_revision();
        }
    }

    /// Undo a deletion: clear the deleted mark so the base object resolves again.
    ///
    /// Unlike [`DeltaLayer::modify_object`] this does not re-add the object to
    /// `modified` — the base copy is intact, and re-serializing it on save
    /// would only duplicate bytes.
    ///
    /// # Example
    /// ```
    /// use pdf_x_core::core::{DeltaLayer, Ref};
    ///
    /// let mut delta = DeltaLayer::new(100);
    /// delta.delete_object(Ref::new(5, 0));
    /// delta.undelete_object(Ref::new(5, 0));
    /// assert!(!delta.is_deleted(&Ref::new(5, 0)));
    /// ```
    pub fn undelete_object(&mut self, obj_ref: Ref) {
        if self.deleted.remove(&(obj_ref.num, obj_ref.generation)) {
            self.bump_revision();
        }
    }

    /// Check if an object is deleted.
    ///
    /// # Arguments
    /// * `obj_ref` - Reference to check
    ///
    /// # Returns
    /// true if the object is marked as deleted
    pub fn is_deleted(&self, obj_ref: &Ref) -> bool {
        self.deleted.contains(&(obj_ref.num, obj_ref.generation))
    }

    /// Get a modified/new object from the delta.
    ///
    /// # Arguments
    /// * `obj_ref` - Reference to the object
    ///
    /// # Returns
    /// Some(DeltaObject) if the object is in the delta, None otherwise
    pub fn get(&self, obj_ref: &Ref) -> Option<&DeltaObject> {
        let key = (obj_ref.num, obj_ref.generation);

        // Check modified objects first
        if let Some(obj) = self.modified.get(&key) {
            return Some(obj);
        }

        // Check new objects
        self.new_objects
            .iter()
            .find(|obj| obj.obj_num == obj_ref.num && obj.generation == obj_ref.generation)
    }

    /// Clear all state differences (reset to the immutable base PDF).
    ///
    /// Command history is intentionally owned separately by [`CommandHistory`];
    /// clearing a serializable delta never mutates a local editor session.
    pub fn clear(&mut self) {
        let changed = !self.modified.is_empty()
            || !self.new_objects.is_empty()
            || !self.deleted.is_empty()
            || !self.trailer_overrides.is_empty()
            || self.next_obj_num != self.base_pdf_size;
        self.modified.clear();
        self.new_objects.clear();
        self.deleted.clear();
        self.trailer_overrides.clear();
        self.next_obj_num = self.base_pdf_size;
        if changed {
            self.bump_revision();
        }
    }

    /// Monotonic revision of the serializable delta state.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Get the total count of changes (modifications + additions + deletions).
    pub fn change_count(&self) -> usize {
        self.modified.len() + self.new_objects.len() + self.deleted.len() + self.trailer_overrides.len()
    }

    /// Get the next object number that will be assigned.
    pub fn next_obj_num(&self) -> u32 {
        self.next_obj_num
    }

    /// Get iterator over modified objects.
    pub fn iter_modified(&self) -> impl Iterator<Item = (&ObjectId, &DeltaObject)> {
        self.modified.iter()
    }

    /// Get iterator over new objects.
    pub fn iter_new_objects(&self) -> impl Iterator<Item = &DeltaObject> {
        self.new_objects.iter()
    }

    /// Get iterator over deleted object references.
    pub fn iter_deleted(&self) -> impl Iterator<Item = &ObjectId> {
        self.deleted.iter()
    }

    /// Override a trailer entry (e.g. `/Info` → new object reference).
    pub fn set_trailer_override(&mut self, key: String, value: PDFObject) {
        if self.trailer_overrides.get(&key) != Some(&value) {
            self.trailer_overrides.insert(key, value);
            self.bump_revision();
        }
    }

    /// Remove a trailer override.
    pub fn remove_trailer_override(&mut self, key: &str) {
        if self.trailer_overrides.remove(key).is_some() {
            self.bump_revision();
        }
    }

    /// Get a trailer override value.
    pub fn get_trailer_override(&self, key: &str) -> Option<&PDFObject> {
        self.trailer_overrides.get(key)
    }

    /// Iterator over trailer overrides.
    pub fn iter_trailer_overrides(&self) -> impl Iterator<Item = (&String, &PDFObject)> {
        self.trailer_overrides.iter()
    }
}

// ========== Common Commands ==========

/// Command to rotate a page.
///
/// This command modifies the /Rotate entry in a page dictionary.
/// The rotation value must be a multiple of 90 (as per PDF spec).
///
/// # Example
/// ```
/// # use pdf_x_core::core::delta::RotatePageCommand;
/// # use pdf_x_core::core::parser::Ref;
/// // Rotate page 5 (object 10) by 90 degrees clockwise
/// let cmd = RotatePageCommand::new(Ref::new(10, 0), 90);
/// ```
#[derive(Debug)]
pub struct RotatePageCommand {
    /// The page object reference to rotate
    page_ref: Ref,

    /// The rotation amount in degrees (must be multiple of 90)
    degrees: u16,

    /// The original rotation value (for undo)
    original_rotation: Option<u16>,

    /// Pre-fetched base page object, set via [`with_original_page`].
    /// When present, `execute` uses this instead of calling `fetch_base`.
    original_page: Option<PDFObject>,
}

impl RotatePageCommand {
    /// Create a new RotatePageCommand.
    ///
    /// # Arguments
    /// * `page_ref` - The object reference of the page to rotate
    /// * `degrees` - The rotation amount (must be multiple of 90: 0, 90, 180, 270)
    ///
    /// # Panics
    /// Panics if degrees is not a multiple of 90
    pub fn new(page_ref: Ref, degrees: u16) -> Self {
        // Validate that degrees is a multiple of 90
        if degrees % 90 != 0 {
            panic!(
                "Page rotation must be a multiple of 90 degrees, got {}",
                degrees
            );
        }

        Self {
            page_ref,
            degrees,
            original_rotation: None,
            original_page: None,
        }
    }

    /// Supply a pre-fetched copy of the base page dictionary.
    ///
    /// When set, `execute` uses this object directly instead of requiring
    /// a `fetch_base` callback. This is the preferred path when executing
    /// through `PDFDocument::execute_command()` (which passes no fetcher).
    pub fn with_original_page(mut self, page: PDFObject) -> Self {
        self.original_page = Some(page);
        self
    }
}

impl Command for RotatePageCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        // Get the current page object: pre-fetched original > delta > base fetch.
        let page_dict = if let Some(ref page) = self.original_page {
            page.clone()
        } else if let Some(delta_obj) = delta.get(&self.page_ref) {
            delta_obj.object.clone()
        } else {
            let fetcher = fetch_base.ok_or_else(|| {
                PDFError::Generic(
                    "Cannot fetch base page object - no fetch callback provided. \
                    Use with_original_page() or execute through PDFDocument."
                        .into(),
                )
            })?;
            fetcher(self.page_ref)?
        };

        // Extract the current dictionary and rotation value
        let (dict, current_rotation) = match page_dict {
            PDFObject::Dictionary(d) => {
                let rotation = d.get("Rotate").and_then(|obj| match obj {
                    PDFObject::Number(n) => Some(*n as u16),
                    _ => None,
                });
                (d, rotation)
            }
            _ => {
                return Err(PDFError::Generic(format!(
                    "Page object {} {} is not a dictionary",
                    self.page_ref.num, self.page_ref.generation
                )));
            }
        };

        // Store original rotation for undo
        self.original_rotation = current_rotation;

        // Clone the dictionary and modify the rotation
        let mut new_dict = dict.clone();
        new_dict.insert("Rotate".to_string(), PDFObject::Number(self.degrees as f64));

        // Modify the page object in delta
        delta.modify_object(self.page_ref, PDFObject::Dictionary(new_dict));

        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // Get the current page object (it should be in delta now since we just modified it)
        let delta_obj = delta.get(&self.page_ref).ok_or_else(|| {
            PDFError::Generic("Page object not found in delta during undo".into())
        })?;

        let mut dict = match &delta_obj.object {
            PDFObject::Dictionary(d) => d.clone(),
            _ => {
                return Err(PDFError::Generic(format!(
                    "Page object {} {} is not a dictionary",
                    self.page_ref.num, self.page_ref.generation
                )));
            }
        };

        // Restore the original rotation value
        if let Some(original) = self.original_rotation {
            dict.insert("Rotate".to_string(), PDFObject::Number(original as f64));
        } else {
            // If there was no original rotation, remove the Rotate key
            dict.remove("Rotate");
        }

        delta.modify_object(self.page_ref, PDFObject::Dictionary(dict));
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // Get the current page object
        let delta_obj = delta.get(&self.page_ref).ok_or_else(|| {
            PDFError::Generic("Page object not found in delta during redo".into())
        })?;

        let mut dict = match &delta_obj.object {
            PDFObject::Dictionary(d) => d.clone(),
            _ => {
                return Err(PDFError::Generic(format!(
                    "Page object {} {} is not a dictionary",
                    self.page_ref.num, self.page_ref.generation
                )));
            }
        };

        // Re-apply the rotation
        dict.insert("Rotate".to_string(), PDFObject::Number(self.degrees as f64));

        delta.modify_object(self.page_ref, PDFObject::Dictionary(dict));
        Ok(())
    }
}

/// Reversible command that sets entries in the document /Info dictionary.
///
/// Each entry is `(key, Some(value))` to set, or `(key, None)` to remove.
/// If the document has no /Info dictionary, one is created.
///
/// Non-ASCII string values are encoded as UTF-16BE with BOM per PDF spec §7.9.2.
pub struct SetInfoCommand {
    /// Pre-fetched original /Info dictionary (if it existed).
    original_info: Option<(Ref, PDFObject)>,
    /// Entries to apply: key → Some(value) means set, key → None means remove.
    entries: Vec<(String, Option<PDFObject>)>,
    /// State captured during execute for undo.
    previous_info_ref: Option<Ref>,
    previous_entries: Option<(Ref, PDFObject)>,
    /// New info ref created during execute (if any).
    new_info_ref: Option<Ref>,
}

impl SetInfoCommand {
    /// Create a new SetInfoCommand.
    ///
    /// `original_info` should be `Some((ref, obj))` if the document currently
    /// has an /Info dictionary, or `None` if it does not.
    pub fn new(
        original_info: Option<(Ref, PDFObject)>,
        entries: Vec<(String, Option<PDFObject>)>,
    ) -> Self {
        Self {
            original_info,
            entries,
            previous_info_ref: None,
            previous_entries: None,
            new_info_ref: None,
        }
    }

    /// Encode a Rust string for a PDF /Info value.
    ///
    /// Per §7.9.2, strings containing non-ASCII characters should be
    /// encoded as UTF-16BE with a leading BOM (\xFE\xFF).
    pub fn encode_info_string(s: &str) -> PDFObject {
        if s.is_ascii() {
            PDFObject::String(s.as_bytes().to_vec())
        } else {
            let mut bytes = vec![0xFE, 0xFF]; // BOM
            for ch in s.encode_utf16() {
                bytes.push((ch >> 8) as u8);
                bytes.push((ch & 0xFF) as u8);
            }
            PDFObject::HexString(bytes)
        }
    }

    fn apply_entries(&self, dict: &mut HashMap<String, PDFObject>) {
        for (key, value) in &self.entries {
            match value {
                Some(v) => {
                    dict.insert(key.clone(), v.clone());
                }
                None => {
                    dict.remove(key);
                }
            }
        }
    }
}

impl Command for SetInfoCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        _fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        // Capture previous state for undo.
        self.previous_info_ref = self.original_info.as_ref().map(|(r, _)| *r);
        self.previous_entries = self.original_info.clone();

        let mut dict = match &self.original_info {
            Some((_ref, PDFObject::Dictionary(d))) => d.clone(),
            Some((_, other)) => {
                return Err(PDFError::Generic(format!(
                    "Existing /Info is not a dictionary: {:?}",
                    other
                )));
            }
            None => HashMap::new(),
        };

        self.apply_entries(&mut dict);

        let info_obj = PDFObject::Dictionary(dict);

        let info_ref = match &self.original_info {
            Some((info_ref, _)) => {
                // Modify existing /Info object.
                delta.modify_object(*info_ref, info_obj.clone());
                *info_ref
            }
            None => {
                // Create new /Info object.
                let r = delta.add_object(info_obj.clone());
                self.new_info_ref = Some(r);
                r
            }
        };

        // Point trailer /Info to the (possibly new) reference.
        delta.set_trailer_override("Info".into(), PDFObject::Ref(info_ref));
        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // Restore the original /Info state.
        match &self.previous_entries {
            Some((info_ref, obj)) => {
                delta.modify_object(*info_ref, obj.clone());
                delta.set_trailer_override(
                    "Info".into(),
                    PDFObject::Ref(*info_ref),
                );
            }
            None => {
                // Remove the /Info we created.
                if let Some(info_ref) = self.new_info_ref {
                    delta.remove_new_object(info_ref);
                }
                delta.remove_trailer_override("Info");
            }
        }
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // Re-apply: rebuild the info dict and set trailer override.
        let mut dict = match &self.previous_entries {
            Some((_, PDFObject::Dictionary(d))) => d.clone(),
            Some((_, other)) => {
                return Err(PDFError::Generic(format!(
                    "Original /Info is not a dictionary: {:?}",
                    other
                )));
            }
            None => HashMap::new(),
        };

        self.apply_entries(&mut dict);

        let info_obj = PDFObject::Dictionary(dict);

        match &self.previous_entries {
            Some((info_ref, _)) => {
                delta.modify_object(*info_ref, info_obj.clone());
                delta.set_trailer_override(
                    "Info".into(),
                    PDFObject::Ref(*info_ref),
                );
            }
            None => {
                let r = self.new_info_ref.ok_or_else(|| {
                    PDFError::Generic("Missing new info ref for redo".into())
                })?;
                delta.restore_new_object(r, info_obj.clone());
                delta.set_trailer_override("Info".into(), PDFObject::Ref(r));
            }
        }
        Ok(())
    }
}

/// Reversible command that deletes a page from the page tree.
///
/// Removes the page's reference from its parent's `/Kids` array and decrements
/// `/Count` on every ancestor up to the root `/Pages` node. The page dict
/// object is additionally marked deleted in the delta, so an incremental save
/// persists it as a free xref entry (spec §7.5.6) instead of leaving a
/// reachable orphan. Undo restores the `/Kids` entry and clears the deletion
/// mark (the base bytes were never touched).
///
/// Reference: pdf.js does not support page deletion at all. Dangling
/// outline/GoTo destinations are tolerated by viewers (v1 limitation).
pub struct DeletePageCommand {
    /// Reference of the page to delete.
    page_ref: Ref,

    /// Reference of the parent `/Pages` node whose `/Kids` contains the page.
    parent_ref: Ref,

    /// Index of the page within the parent's `/Kids` array.
    kid_index: usize,

    /// Ancestor chain from parent up to root: `(ref, original_count)`.
    ancestors: Vec<(Ref, u32)>,

    /// Pre-fetched parent dictionary (contains `/Kids`).
    original_parent: PDFObject,

    /// Pre-fetched ancestor dictionaries (for `/Count` restoration).
    original_ancestors: Vec<PDFObject>,
}

impl DeletePageCommand {
    /// Create a new page-deletion command.
    ///
    /// All objects are pre-fetched so the command needs no `fetch_base` callback.
    /// Undo restores page resolution via [`DeltaLayer::undelete_object`] — the
    /// base bytes of the page dict are never touched.
    pub fn new(
        page_ref: Ref,
        parent_ref: Ref,
        kid_index: usize,
        ancestors: Vec<(Ref, u32)>,
        original_parent: PDFObject,
        original_ancestors: Vec<PDFObject>,
    ) -> Self {
        Self {
            page_ref,
            parent_ref,
            kid_index,
            ancestors,
            original_parent,
            original_ancestors,
        }
    }
}

impl Command for DeletePageCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        _fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        // 1. Remove page ref from parent's /Kids and decrement parent's /Count in a single write.
        let parent_dict = match &self.original_parent {
            PDFObject::Dictionary(d) => d.clone(),
            _ => {
                return Err(PDFError::Generic(format!(
                    "Parent {} {} is not a dictionary",
                    self.parent_ref.num, self.parent_ref.generation
                )));
            }
        };

        let kids = parent_dict
            .get("Kids")
            .and_then(|k| k.as_array())
            .ok_or_else(|| {
                PDFError::Generic(format!(
                    "Parent {} {} has no Kids array",
                    self.parent_ref.num, self.parent_ref.generation
                ))
            })?;

        let mut new_kids: Vec<Box<PDFObject>> = kids.to_vec();
        if self.kid_index >= new_kids.len() {
            return Err(PDFError::Generic(format!(
                "kid_index {} out of range (Kids len={})",
                self.kid_index,
                new_kids.len()
            )));
        }
        new_kids.remove(self.kid_index);

        // Decrement /Count on the parent too.
        let parent_count = parent_dict
            .get("Count")
            .and_then(|c| c.as_number())
            .unwrap_or(0.0) as u32;

        let mut new_parent = parent_dict;
        new_parent.insert(
            "Kids".to_string(),
            PDFObject::Array(new_kids.into()),
        );
        new_parent.insert(
            "Count".to_string(),
            PDFObject::Number((parent_count.saturating_sub(1)) as f64),
        );
        delta.modify_object(self.parent_ref, PDFObject::Dictionary(new_parent));

        // 2. Decrement /Count on ancestors ABOVE the parent (excluding parent).
        for ((ancestor_ref, _original_count), original_obj) in
            self.ancestors.iter().zip(self.original_ancestors.iter())
        {
            // Skip the parent itself — already handled above.
            if ancestor_ref.num == self.parent_ref.num
                && ancestor_ref.generation == self.parent_ref.generation
            {
                continue;
            }
            let dict = match original_obj {
                PDFObject::Dictionary(d) => d.clone(),
                _ => continue,
            };
            let current_count = dict
                .get("Count")
                .and_then(|c| c.as_number())
                .unwrap_or(0.0) as u32;
            let mut new_dict = dict;
            new_dict.insert(
                "Count".to_string(),
                PDFObject::Number((current_count.saturating_sub(1)) as f64),
            );
            delta.modify_object(*ancestor_ref, PDFObject::Dictionary(new_dict));
        }

        // 3. Mark the page dict itself deleted so an incremental save persists
        // it as a free xref entry (spec §7.5.6) instead of a reachable orphan.
        delta.delete_object(self.page_ref);

        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // 1. Re-insert page ref into parent's /Kids and restore parent's /Count.
        let parent_delta = delta
            .get(&self.parent_ref)
            .ok_or_else(|| PDFError::Generic("Parent not in delta during undo".into()))?;

        let mut parent_dict = match &parent_delta.object {
            PDFObject::Dictionary(d) => d.clone(),
            _ => {
                return Err(PDFError::Generic(
                    "Parent in delta is not a dictionary".into(),
                ));
            }
        };

        let kids = parent_dict
            .get("Kids")
            .and_then(|k| k.as_array())
            .ok_or_else(|| PDFError::Generic("Parent has no Kids array in undo".into()))?
            .to_vec();

        let mut new_kids: Vec<Box<PDFObject>> = kids;
        new_kids.insert(
            self.kid_index,
            Box::new(PDFObject::Ref(self.page_ref)),
        );

        // Restore parent's /Count from original.
        let original_parent_count = match &self.original_parent {
            PDFObject::Dictionary(d) => d
                .get("Count")
                .and_then(|c| c.as_number())
                .unwrap_or(0.0) as u32,
            _ => 0,
        };
        parent_dict.insert(
            "Kids".to_string(),
            PDFObject::Array(new_kids.into()),
        );
        parent_dict.insert(
            "Count".to_string(),
            PDFObject::Number(original_parent_count as f64),
        );
        delta.modify_object(self.parent_ref, PDFObject::Dictionary(parent_dict));

        // 2. Restore /Count on ancestors above the parent.
        for ((ancestor_ref, original_count), _original_obj) in
            self.ancestors.iter().zip(self.original_ancestors.iter())
        {
            if ancestor_ref.num == self.parent_ref.num
                && ancestor_ref.generation == self.parent_ref.generation
            {
                continue;
            }
            // Read the current ancestor from delta (it was modified in execute).
            let ancestor_delta = delta
                .get(ancestor_ref)
                .ok_or_else(|| {
                    PDFError::Generic(format!(
                        "Ancestor {} {} not in delta during undo",
                        ancestor_ref.num, ancestor_ref.generation
                    ))
                })?;

            let mut dict = match &ancestor_delta.object {
                PDFObject::Dictionary(d) => d.clone(),
                _ => continue,
            };
            dict.insert(
                "Count".to_string(),
                PDFObject::Number(*original_count as f64),
            );
            delta.modify_object(*ancestor_ref, PDFObject::Dictionary(dict));
        }

        // 3. Clear the page dict's deletion mark — the base bytes are intact,
        // so the object resolves again without re-serializing anything.
        delta.undelete_object(self.page_ref);

        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // redo is identical to execute: remove kid, decrement counts.
        self.execute(delta, None)
    }
}

/// Reversible command that moves a page to a new slot in the page tree.
///
/// The page is removed from the source parent's `/Kids` at `src_kid_index`
/// and inserted into the destination parent's `/Kids` at `dst_insert_at`.
/// A same-parent move is one merged `/Kids` permutation with no `/Count`
/// write (the count is net zero); a cross-parent move decrements `/Count`
/// on the source chain and increments it on the destination chain. Ancestor
/// chains can share a tail (the common path up to the root `/Pages`): those
/// nodes get a net-zero delta and are left untouched.
///
/// Reference: pdf.js has no page-move support (viewer-level feature, like
/// deletion). hayro only extracts pages into a *new* PDF
/// (`hayro-write/src/lib.rs` `extract_pages_to_pdf`), not in-place moves.
pub struct MovePageCommand {
    /// Reference of the page dict being moved.
    page_ref: Ref,

    /// Reference of the parent `/Pages` node currently holding the page.
    src_parent_ref: Ref,

    /// Slot of the page within the source parent's `/Kids`.
    src_kid_index: usize,

    /// Pre-fetched source parent dictionary.
    original_src_parent: PDFObject,

    /// Ancestors strictly above the source parent: `(ref, original dict)`.
    src_ancestors: Vec<(Ref, PDFObject)>,

    /// Reference of the destination parent `/Pages` node.
    dst_parent_ref: Ref,

    /// Insert slot within the destination parent's `/Kids`.
    dst_insert_at: usize,

    /// Pre-fetched destination parent dictionary.
    original_dst_parent: PDFObject,

    /// Ancestors strictly above the destination parent: `(ref, original dict)`.
    dst_ancestors: Vec<(Ref, PDFObject)>,

    /// Whether both endpoints resolve to the same parent node.
    same_parent: bool,
}

impl MovePageCommand {
    /// Create a new page-move command.
    ///
    /// All objects are pre-fetched so the command needs no `fetch_base`
    /// callback. `src_ancestors` / `dst_ancestors` must exclude the parents
    /// themselves (chain strictly above each parent).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        page_ref: Ref,
        src_parent_ref: Ref,
        src_kid_index: usize,
        original_src_parent: PDFObject,
        src_ancestors: Vec<(Ref, PDFObject)>,
        dst_parent_ref: Ref,
        dst_insert_at: usize,
        original_dst_parent: PDFObject,
        dst_ancestors: Vec<(Ref, PDFObject)>,
        same_parent: bool,
    ) -> Self {
        Self {
            page_ref,
            src_parent_ref,
            src_kid_index,
            original_src_parent,
            src_ancestors,
            dst_parent_ref,
            dst_insert_at,
            original_dst_parent,
            dst_ancestors,
            same_parent,
        }
    }

    /// True if `r` sits on the source ancestor chain.
    fn on_src_chain(&self, r: &Ref) -> bool {
        self.src_ancestors
            .iter()
            .any(|(a, _)| a.num == r.num && a.generation == r.generation)
    }

    /// True if `r` sits on the destination ancestor chain.
    fn on_dst_chain(&self, r: &Ref) -> bool {
        self.dst_ancestors
            .iter()
            .any(|(a, _)| a.num == r.num && a.generation == r.generation)
    }
}

impl Command for MovePageCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        _fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        if self.same_parent {
            // Same parent: one merged write — a pure /Kids permutation. The
            // parent's /Count is net zero and must not be rewritten.
            let mut parent = match &self.original_src_parent {
                PDFObject::Dictionary(d) => d.clone(),
                _ => {
                    return Err(PDFError::Generic(format!(
                        "Source parent {} {} is not a dictionary",
                        self.src_parent_ref.num, self.src_parent_ref.generation
                    )));
                }
            };
            let mut kids = parent
                .get("Kids")
                .and_then(|k| k.as_array())
                .ok_or_else(|| {
                    PDFError::Generic(format!(
                        "Source parent {} {} has no Kids array",
                        self.src_parent_ref.num, self.src_parent_ref.generation
                    ))
                })?
                .to_vec();
            if self.src_kid_index >= kids.len() {
                return Err(PDFError::Generic(format!(
                    "src_kid_index {} out of range (Kids len={})",
                    self.src_kid_index,
                    kids.len()
                )));
            }
            if self.dst_insert_at > kids.len() - 1 {
                return Err(PDFError::Generic(format!(
                    "dst_insert_at {} out of range (post-removal Kids len={})",
                    self.dst_insert_at,
                    kids.len() - 1
                )));
            }
            let page = kids.remove(self.src_kid_index);
            kids.insert(self.dst_insert_at, page);
            parent.insert("Kids".to_string(), PDFObject::Array(kids.into()));
            delta.modify_object(self.src_parent_ref, PDFObject::Dictionary(parent));
            return Ok(());
        }

        // Cross-parent move: remove from the source parent (Count -1) and
        // insert into the destination parent (Count +1) as two writes.
        let mut src = match &self.original_src_parent {
            PDFObject::Dictionary(d) => d.clone(),
            _ => {
                return Err(PDFError::Generic(format!(
                    "Source parent {} {} is not a dictionary",
                    self.src_parent_ref.num, self.src_parent_ref.generation
                )));
            }
        };
        let mut src_kids = src
            .get("Kids")
            .and_then(|k| k.as_array())
            .ok_or_else(|| {
                PDFError::Generic(format!(
                    "Source parent {} {} has no Kids array",
                    self.src_parent_ref.num, self.src_parent_ref.generation
                ))
            })?
            .to_vec();
        if self.src_kid_index >= src_kids.len() {
            return Err(PDFError::Generic(format!(
                "src_kid_index {} out of range (Kids len={})",
                self.src_kid_index,
                src_kids.len()
            )));
        }
        let _page = src_kids.remove(self.src_kid_index);
        let src_count = src.get("Count").and_then(|c| c.as_number()).unwrap_or(0.0) as u32;
        src.insert("Kids".to_string(), PDFObject::Array(src_kids.into()));
        src.insert(
            "Count".to_string(),
            PDFObject::Number(src_count.saturating_sub(1) as f64),
        );
        delta.modify_object(self.src_parent_ref, PDFObject::Dictionary(src));

        let mut dst = match &self.original_dst_parent {
            PDFObject::Dictionary(d) => d.clone(),
            _ => {
                return Err(PDFError::Generic(format!(
                    "Destination parent {} {} is not a dictionary",
                    self.dst_parent_ref.num, self.dst_parent_ref.generation
                )));
            }
        };
        let mut dst_kids = dst
            .get("Kids")
            .and_then(|k| k.as_array())
            .ok_or_else(|| {
                PDFError::Generic(format!(
                    "Destination parent {} {} has no Kids array",
                    self.dst_parent_ref.num, self.dst_parent_ref.generation
                ))
            })?
            .to_vec();
        if self.dst_insert_at > dst_kids.len() {
            return Err(PDFError::Generic(format!(
                "dst_insert_at {} out of range (Kids len={})",
                self.dst_insert_at,
                dst_kids.len()
            )));
        }
        dst_kids.insert(self.dst_insert_at, Box::new(PDFObject::Ref(self.page_ref)));
        let dst_count = dst.get("Count").and_then(|c| c.as_number()).unwrap_or(0.0) as u32;
        dst.insert("Kids".to_string(), PDFObject::Array(dst_kids.into()));
        dst.insert(
            "Count".to_string(),
            PDFObject::Number(dst_count.saturating_add(1) as f64),
        );
        delta.modify_object(self.dst_parent_ref, PDFObject::Dictionary(dst));

        // Ancestor /Count walks. Shared-tail ancestors appear on both chains:
        // -1 and +1 cancel out, so they are skipped on BOTH sides rather than
        // written (a lone -1 would corrupt the shared count).
        for (ancestor_ref, original) in &self.src_ancestors {
            if self.on_dst_chain(ancestor_ref) {
                continue;
            }
            if let PDFObject::Dictionary(d) = original {
                let mut dict = d.clone();
                let count = dict.get("Count").and_then(|c| c.as_number()).unwrap_or(0.0) as u32;
                dict.insert(
                    "Count".to_string(),
                    PDFObject::Number(count.saturating_sub(1) as f64),
                );
                delta.modify_object(*ancestor_ref, PDFObject::Dictionary(dict));
            }
        }
        for (ancestor_ref, original) in &self.dst_ancestors {
            if self.on_src_chain(ancestor_ref) {
                continue;
            }
            if let PDFObject::Dictionary(d) = original {
                let mut dict = d.clone();
                let count = dict.get("Count").and_then(|c| c.as_number()).unwrap_or(0.0) as u32;
                dict.insert(
                    "Count".to_string(),
                    PDFObject::Number(count.saturating_add(1) as f64),
                );
                delta.modify_object(*ancestor_ref, PDFObject::Dictionary(dict));
            }
        }

        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // Undo runs LIFO, so the delta holds exactly the post-execute state:
        // restoring every pre-fetched original is sufficient.
        delta.modify_object(self.src_parent_ref, self.original_src_parent.clone());
        if !self.same_parent {
            delta.modify_object(self.dst_parent_ref, self.original_dst_parent.clone());
        }
        for (ancestor_ref, original) in &self.src_ancestors {
            if self.on_dst_chain(ancestor_ref) {
                continue; // never written by execute
            }
            delta.modify_object(*ancestor_ref, original.clone());
        }
        for (ancestor_ref, original) in &self.dst_ancestors {
            if self.on_src_chain(ancestor_ref) {
                continue; // never written by execute
            }
            delta.modify_object(*ancestor_ref, original.clone());
        }
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        // redo is identical to execute: re-apply the move.
        self.execute(delta, None)
    }
}

/// Reversible command that appends a standard highlight annotation to a page.
pub struct AddHighlightCommand {
    page_ref: Ref,
    original_page: PDFObject,
    rect: [f64; 4],
    color: [f64; 3],
    annotation_ref: Option<Ref>,
    appearance_ref: Option<Ref>,
    annotation: Option<PDFObject>,
    appearance: Option<PDFObject>,
    modified_page: Option<PDFObject>,
}

impl AddHighlightCommand {
    /// Create a page highlight command using PDF user-space coordinates.
    pub fn new(
        page_ref: Ref,
        original_page: PDFObject,
        rect: [f64; 4],
        color: [f64; 3],
    ) -> PDFResult<Self> {
        if rect[2] <= rect[0] || rect[3] <= rect[1] {
            return Err(PDFError::Generic(
                "Highlight rectangle must have positive size".into(),
            ));
        }
        if !matches!(original_page, PDFObject::Dictionary(_)) {
            return Err(PDFError::Generic("Page object is not a dictionary".into()));
        }
        Ok(Self {
            page_ref,
            original_page,
            rect,
            color,
            annotation_ref: None,
            appearance_ref: None,
            annotation: None,
            appearance: None,
            modified_page: None,
        })
    }

    fn number_array(values: &[f64]) -> PDFObject {
        PDFObject::Array(
            values
                .iter()
                .map(|n| Box::new(PDFObject::Number(*n)))
                .collect(),
        )
    }

    fn build_objects(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        let width = self.rect[2] - self.rect[0];
        let height = self.rect[3] - self.rect[1];
        let content = format!(
            "q {} {} {} rg 0 0 {} {} re f Q\n",
            self.color[0], self.color[1], self.color[2], width, height
        )
        .into_bytes();
        let mut ap_dict = HashMap::new();
        ap_dict.insert("Type".into(), PDFObject::Name("XObject".into()));
        ap_dict.insert("Subtype".into(), PDFObject::Name("Form".into()));
        ap_dict.insert(
            "BBox".into(),
            Self::number_array(&[0.0, 0.0, width, height]),
        );
        ap_dict.insert("Length".into(), PDFObject::Number(content.len() as f64));
        let appearance = PDFObject::Stream {
            dict: ap_dict,
            data: content,
        };
        let appearance_ref = delta.add_object(appearance.clone());

        let mut normal_ap = HashMap::new();
        normal_ap.insert("N".into(), PDFObject::Ref(appearance_ref));
        let mut annot_dict = HashMap::new();
        annot_dict.insert("Type".into(), PDFObject::Name("Annot".into()));
        annot_dict.insert("Subtype".into(), PDFObject::Name("Highlight".into()));
        annot_dict.insert("Rect".into(), Self::number_array(&self.rect));
        annot_dict.insert(
            "QuadPoints".into(),
            Self::number_array(&[
                self.rect[0],
                self.rect[3],
                self.rect[2],
                self.rect[3],
                self.rect[0],
                self.rect[1],
                self.rect[2],
                self.rect[1],
            ]),
        );
        annot_dict.insert("C".into(), Self::number_array(&self.color));
        annot_dict.insert("F".into(), PDFObject::Number(4.0));
        annot_dict.insert("AP".into(), PDFObject::Dictionary(normal_ap));
        let annotation = PDFObject::Dictionary(annot_dict);
        let annotation_ref = delta.add_object(annotation.clone());

        let mut page_dict = match &self.original_page {
            PDFObject::Dictionary(dict) => dict.clone(),
            _ => unreachable!(),
        };
        let mut annots = match page_dict.remove("Annots") {
            Some(PDFObject::Array(items)) => items,
            Some(other) => smallvec::smallvec![Box::new(other)],
            None => smallvec::SmallVec::new(),
        };
        annots.push(Box::new(PDFObject::Ref(annotation_ref)));
        page_dict.insert("Annots".into(), PDFObject::Array(annots));
        let modified_page = PDFObject::Dictionary(page_dict);
        delta.modify_object(self.page_ref, modified_page.clone());

        self.annotation_ref = Some(annotation_ref);
        self.appearance_ref = Some(appearance_ref);
        self.annotation = Some(annotation);
        self.appearance = Some(appearance);
        self.modified_page = Some(modified_page);
        Ok(())
    }
}

impl Command for AddHighlightCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        _fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        self.build_objects(delta)
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        delta.modify_object(self.page_ref, self.original_page.clone());
        if let Some(reference) = self.annotation_ref {
            delta.remove_new_object(reference);
        }
        if let Some(reference) = self.appearance_ref {
            delta.remove_new_object(reference);
        }
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        let annotation_ref = self
            .annotation_ref
            .ok_or_else(|| PDFError::Generic("Missing highlight annotation reference".into()))?;
        let appearance_ref = self
            .appearance_ref
            .ok_or_else(|| PDFError::Generic("Missing highlight appearance reference".into()))?;
        delta.restore_new_object(appearance_ref, self.appearance.clone().unwrap());
        delta.restore_new_object(annotation_ref, self.annotation.clone().unwrap());
        delta.modify_object(self.page_ref, self.modified_page.clone().unwrap());
        Ok(())
    }
}

// ── ReplaceTextCommand ─────────────────────────────────────────────

/// Pre-computed text replacement for one or more content-stream fragments.
///
/// All encoding work (inverse map, code generation, serialization) is
/// done in [`ReplaceTextCommand::new`] so that `execute`/`undo`/`redo`
/// are simple `modify_object` calls.
pub struct ReplaceTextCommand {
    /// (stream ref, original stream object, replacement stream object).
    /// execute writes `replacement`; undo writes `original`.
    fragments: Vec<(Ref, PDFObject, PDFObject)>,
}

impl ReplaceTextCommand {
    /// Create a new command from pre-built stream objects.
    ///
    /// Each entry is `(stream_ref, original_stream, replacement_stream)`.
    /// Both stream objects must be `PDFObject::Stream` with proper `/Length`.
    pub fn new(fragments: Vec<(Ref, PDFObject, PDFObject)>) -> Self {
        Self { fragments }
    }
}

impl Command for ReplaceTextCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        _fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        for (ref_, _original, replacement) in &self.fragments {
            delta.modify_object(*ref_, replacement.clone());
        }
        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        for (ref_, original, _replacement) in &self.fragments {
            delta.modify_object(*ref_, original.clone());
        }
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        for (ref_, _original, replacement) in &self.fragments {
            delta.modify_object(*ref_, replacement.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_layer_creation() {
        let delta = DeltaLayer::new(100);
        let history = CommandHistory::new();
        assert_eq!(delta.next_obj_num(), 100);
        assert_eq!(delta.change_count(), 0);
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn revision_tracks_effective_mutations() {
        let mut delta = DeltaLayer::new(100);
        let object_ref = Ref::new(5, 0);

        assert_eq!(delta.revision(), 0);

        delta.modify_object(object_ref, PDFObject::Number(42.0));
        assert_eq!(delta.revision(), 1);

        // Re-applying the exact same state is not a new document revision.
        delta.modify_object(object_ref, PDFObject::Number(42.0));
        assert_eq!(delta.revision(), 1);

        delta.delete_object(object_ref);
        assert_eq!(delta.revision(), 2);

        // Re-applying an existing deletion is also a no-op.
        delta.delete_object(object_ref);
        assert_eq!(delta.revision(), 2);

        delta.clear();
        assert_eq!(delta.revision(), 3);

        // Clearing an already-empty delta does not invalidate caches again.
        delta.clear();
        assert_eq!(delta.revision(), 3);
    }

    #[test]
    fn test_add_object() {
        let mut delta = DeltaLayer::new(100);

        let ref1 = delta.add_object(PDFObject::Number(42.0));
        assert_eq!(ref1.num, 100);
        assert_eq!(ref1.generation, 0);
        assert_eq!(delta.next_obj_num(), 101);
        assert_eq!(delta.change_count(), 1);

        let ref2 = delta.add_object(PDFObject::String(b"hello".to_vec()));
        assert_eq!(ref2.num, 101);
        assert_eq!(delta.next_obj_num(), 102);
        assert_eq!(delta.change_count(), 2);
    }

    #[test]
    fn test_modify_object() {
        let mut delta = DeltaLayer::new(100);
        let obj_ref = Ref::new(5, 0);

        delta.modify_object(obj_ref, PDFObject::Number(42.0));

        // Check that it's in the delta
        let retrieved = delta.get(&obj_ref);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().object, PDFObject::Number(42.0));
        assert_eq!(delta.change_count(), 1);
    }

    #[test]
    fn test_delete_object() {
        let mut delta = DeltaLayer::new(100);
        let obj_ref = Ref::new(5, 0);

        delta.delete_object(obj_ref);

        assert!(delta.is_deleted(&obj_ref));
        assert_eq!(delta.change_count(), 1);
    }

    #[test]
    fn undelete_object_restores_base_resolution() {
        let mut delta = DeltaLayer::new(100);
        let obj_ref = Ref::new(5, 0);
        delta.modify_object(obj_ref, PDFObject::Number(42.0));
        delta.delete_object(obj_ref);
        assert!(delta.is_deleted(&obj_ref));
        let revision_after_delete = delta.revision();

        delta.undelete_object(obj_ref);

        assert!(!delta.is_deleted(&obj_ref));
        assert!(
            delta.iter_deleted().next().is_none(),
            "deletion must be gone from the serializable state"
        );
        // Undeleting must NOT re-add the object to `modified`: the base copy
        // is intact and re-serializing it on save would duplicate bytes.
        assert!(
            delta.get(&obj_ref).is_none(),
            "undelete must not resurrect the object as a modification"
        );
        // Note: delete_object already dropped the earlier modification from
        // `modified` (deletion supersedes modification), so the delta is now
        // fully empty — undelete must not fabricate a modification either.
        assert_eq!(delta.change_count(), 0, "delta must be fully empty");
        assert!(
            delta.revision() > revision_after_delete,
            "undelete is an effective mutation and must bump the revision"
        );

        // Undeleting a live (never-deleted) object is a no-op.
        let before = delta.revision();
        delta.undelete_object(Ref::new(6, 0));
        assert_eq!(delta.revision(), before);
    }

    #[test]
    fn delete_page_command_marks_page_deleted_and_undo_restores() {
        // Parent /Pages with two kids; delete the second.
        let mut parent = std::collections::HashMap::new();
        parent.insert("Type".to_string(), PDFObject::Name("Pages".to_string()));
        parent.insert("Count".to_string(), PDFObject::Number(2.0));
        parent.insert(
            "Kids".to_string(),
            PDFObject::Array(
                vec![
                    Box::new(PDFObject::Ref(Ref::new(4, 0))),
                    Box::new(PDFObject::Ref(Ref::new(8, 0))),
                ]
                .into(),
            ),
        );

        let mut delta = DeltaLayer::new(100);
        let mut cmd = DeletePageCommand::new(
            Ref::new(8, 0),
            Ref::new(2, 0),
            1,
            Vec::new(),
            PDFObject::Dictionary(parent.clone()),
            Vec::new(),
        );

        cmd.execute(&mut delta, None).expect("delete must execute");

        // The page ref left /Kids AND is marked deleted for persistence.
        assert!(delta.is_deleted(&Ref::new(8, 0)));
        let deleted: Vec<_> = delta.iter_deleted().copied().collect();
        assert_eq!(deleted, vec![(8, 0)]);

        cmd.undo(&mut delta).expect("undo must restore");

        assert!(
            !delta.is_deleted(&Ref::new(8, 0)),
            "undo must clear the deletion mark"
        );
        assert!(delta.iter_deleted().next().is_none());
    }

    #[test]
    fn test_delete_then_modify() {
        let mut delta = DeltaLayer::new(100);
        let obj_ref = Ref::new(5, 0);

        // Delete first
        delta.delete_object(obj_ref);
        assert!(delta.is_deleted(&obj_ref));

        // Modify should remove from deleted set
        delta.modify_object(obj_ref, PDFObject::Number(42.0));
        assert!(!delta.is_deleted(&obj_ref));

        let retrieved = delta.get(&obj_ref);
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_clear() {
        let mut delta = DeltaLayer::new(100);

        delta.add_object(PDFObject::Number(42.0));
        delta.modify_object(Ref::new(5, 0), PDFObject::Null);
        delta.delete_object(Ref::new(10, 0));

        assert_eq!(delta.change_count(), 3);

        delta.clear();

        assert_eq!(delta.change_count(), 0);
        assert_eq!(delta.next_obj_num(), 100); // Should preserve base size
    }

    #[test]
    fn test_get_new_object() {
        let mut delta = DeltaLayer::new(100);

        let ref1 = delta.add_object(PDFObject::Number(42.0));
        let retrieved = delta.get(&ref1);

        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().object, PDFObject::Number(42.0));
    }

    #[test]
    fn test_get_nonexistent_object() {
        let delta = DeltaLayer::new(100);
        let obj_ref = Ref::new(5, 0);

        let retrieved = delta.get(&obj_ref);
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_change_count() {
        let mut delta = DeltaLayer::new(100);

        assert_eq!(delta.change_count(), 0);

        delta.add_object(PDFObject::Null);
        assert_eq!(delta.change_count(), 1);

        delta.modify_object(
            Ref {
                num: 5,
                generation: 0,
            },
            PDFObject::Null,
        );
        assert_eq!(delta.change_count(), 2);

        delta.delete_object(Ref {
            num: 10,
            generation: 0,
        });
        assert_eq!(delta.change_count(), 3);
    }
}
