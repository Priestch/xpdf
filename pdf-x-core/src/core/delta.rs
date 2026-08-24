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
}

impl std::fmt::Debug for DeltaLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeltaLayer")
            .field("modified", &self.modified)
            .field("new_objects", &self.new_objects)
            .field("deleted", &self.deleted)
            .field("next_obj_num", &self.next_obj_num)
            .field("base_pdf_size", &self.base_pdf_size)
            .finish()
    }
}

/// Object in the delta layer with metadata.
#[derive(Debug, Clone)]
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

        // Remove from deleted set if it was marked as deleted
        self.deleted.remove(&key);

        // Add to modified map
        self.modified.insert(
            key,
            DeltaObject {
                object: new_obj,
                obj_num: obj_ref.num,
                generation: obj_ref.generation,
            },
        );
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

        Ref::new(obj_num, 0)
    }

    /// Restore an object at a previously allocated reference (used by redo).
    pub fn restore_new_object(&mut self, obj_ref: Ref, obj: PDFObject) {
        self.deleted.remove(&obj_ref.as_id());
        if let Some(existing) = self
            .new_objects
            .iter_mut()
            .find(|item| item.obj_num == obj_ref.num && item.generation == obj_ref.generation)
        {
            existing.object = obj;
            return;
        }
        self.new_objects.push(DeltaObject {
            object: obj,
            obj_num: obj_ref.num,
            generation: obj_ref.generation,
        });
        self.next_obj_num = self.next_obj_num.max(obj_ref.num + 1);
    }

    /// Remove a newly-created object while retaining its object number for redo.
    pub fn remove_new_object(&mut self, obj_ref: Ref) {
        self.new_objects
            .retain(|item| item.obj_num != obj_ref.num || item.generation != obj_ref.generation);
        self.deleted.remove(&obj_ref.as_id());
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
        self.deleted.insert(key);

        // Remove from modified if it was previously modified
        self.modified.remove(&key);
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
        self.modified.clear();
        self.new_objects.clear();
        self.deleted.clear();
        self.next_obj_num = self.base_pdf_size;
    }

    /// Get the total count of changes (modifications + additions + deletions).
    pub fn change_count(&self) -> usize {
        self.modified.len() + self.new_objects.len() + self.deleted.len()
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
        }
    }
}

impl Command for RotatePageCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> PDFResult<()> {
        // Get the current page object from delta or base PDF
        let page_dict = match delta.get(&self.page_ref) {
            Some(delta_obj) => {
                // Page is already in delta (modified or new)
                delta_obj.object.clone()
            }
            None => {
                // Page not in delta - fetch from base PDF
                let fetcher = fetch_base.ok_or_else(|| {
                    PDFError::Generic(
                        "Cannot fetch base page object - no fetch callback provided. \
                        Execute commands through PDFDocument::execute_command() instead."
                            .into(),
                    )
                })?;

                fetcher(self.page_ref)?
            }
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
