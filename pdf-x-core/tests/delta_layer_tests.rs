//! Delta layer tests.
//!
//! Tests for the delta layer that tracks PDF document modifications.

use pdf_x_core::core::delta::{BaseObjectFetcher, Command, CommandHistory};
use pdf_x_core::core::{
    DeltaLayer,
    parser::{PDFObject, Ref},
};

// Simple test command that modifies an object
struct ModifyCommand {
    obj_ref: Ref,
    old_value: Option<PDFObject>,
    new_value: PDFObject,
    had_old_value: bool, // Track if we had an old value
}

impl Command for ModifyCommand {
    fn execute<'a>(
        &mut self,
        delta: &mut DeltaLayer,
        _fetch_base: Option<&'a BaseObjectFetcher<'a>>,
    ) -> pdf_x_core::core::error::PDFResult<()> {
        // Store old value for undo
        if let Some(obj) = delta.get(&self.obj_ref) {
            self.old_value = Some(obj.object.clone());
            self.had_old_value = true;
        } else {
            self.had_old_value = false;
        }

        delta.modify_object(self.obj_ref, self.new_value.clone());
        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> pdf_x_core::core::error::PDFResult<()> {
        if self.had_old_value {
            // Object existed before, restore it
            if let Some(old_value) = &self.old_value {
                delta.modify_object(self.obj_ref, old_value.clone());
            }
        } else {
            // Object didn't exist before, mark as deleted to remove modification
            // This is the correct behavior - undoing a modification to a new object
            // should mark it as deleted
            delta.delete_object(self.obj_ref);
        }
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> pdf_x_core::core::error::PDFResult<()> {
        delta.modify_object(self.obj_ref, self.new_value.clone());
        Ok(())
    }
}

#[test]
fn test_delta_layer_creation() {
    let history = CommandHistory::new();
    let delta = DeltaLayer::new(100);

    assert_eq!(delta.change_count(), 0);
    assert!(!history.can_undo());
    assert!(!history.can_redo());
}

#[test]
fn test_modify_object() {
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);
    let new_obj = PDFObject::Number(42.0);

    delta.modify_object(obj_ref, new_obj.clone());

    // Verify object is in delta
    let retrieved = delta.get(&obj_ref);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().object, new_obj);
    assert_eq!(delta.change_count(), 1);
}

#[test]
fn test_modify_then_delete_removes_from_modified() {
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);
    let new_obj = PDFObject::Number(42.0);

    // First modify
    delta.modify_object(obj_ref, new_obj);
    assert_eq!(delta.change_count(), 1);

    // Then delete - should remove from modified map and add to deleted
    delta.delete_object(obj_ref);
    assert!(delta.is_deleted(&obj_ref));
    assert!(delta.get(&obj_ref).is_none());
}

#[test]
fn test_add_new_object() {
    let mut delta = DeltaLayer::new(100);

    let obj1 = PDFObject::Number(1.0);
    let ref1 = delta.add_object(obj1);

    assert_eq!(ref1.num, 100); // First new object gets number 100
    assert_eq!(ref1.generation, 0);

    let obj2 = PDFObject::Number(2.0);
    let ref2 = delta.add_object(obj2);

    assert_eq!(ref2.num, 101); // Second new object gets number 101
    assert_eq!(delta.change_count(), 2);
}

#[test]
fn test_get_new_object() {
    let mut delta = DeltaLayer::new(100);

    let obj = PDFObject::String(b"test".to_vec());
    let obj_ref = delta.add_object(obj.clone());

    let retrieved = delta.get(&obj_ref);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().object, obj);
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
fn test_delete_removes_from_modified() {
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);

    // Modify first
    delta.modify_object(obj_ref, PDFObject::Number(42.0));
    assert!(delta.get(&obj_ref).is_some());

    // Delete - should remove from modified
    delta.delete_object(obj_ref);
    assert!(delta.get(&obj_ref).is_none());
    assert!(delta.is_deleted(&obj_ref));
}

#[test]
fn test_command_execution() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);
    let new_value = PDFObject::Number(42.0);

    let cmd = Box::new(ModifyCommand {
        obj_ref,
        old_value: None,
        new_value,
        had_old_value: false,
    });

    history.execute(&mut delta, cmd, None).unwrap();

    assert!(history.can_undo());
    assert!(!history.can_redo());
    assert_eq!(delta.change_count(), 1);

    let retrieved = delta.get(&obj_ref);
    assert!(retrieved.is_some());
}

#[test]
fn test_command_undo() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);
    let new_value = PDFObject::Number(42.0);

    let cmd = Box::new(ModifyCommand {
        obj_ref,
        old_value: None,
        new_value,
        had_old_value: false,
    });

    history.execute(&mut delta, cmd, None).unwrap();
    assert!(delta.get(&obj_ref).is_some());

    // Undo
    history.undo(&mut delta).unwrap();
    assert!(!history.can_undo());
    assert!(history.can_redo());
    assert!(delta.get(&obj_ref).is_none()); // Object removed after undo
}

#[test]
fn test_command_redo() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);
    let new_value = PDFObject::Number(42.0);

    let cmd = Box::new(ModifyCommand {
        obj_ref,
        old_value: None,
        new_value,
        had_old_value: false,
    });

    history.execute(&mut delta, cmd, None).unwrap();

    // Undo
    history.undo(&mut delta).unwrap();
    assert!(delta.get(&obj_ref).is_none());

    // Redo
    history.redo(&mut delta).unwrap();
    assert!(history.can_undo());
    assert!(!history.can_redo());
    assert!(delta.get(&obj_ref).is_some()); // Object back after redo
}

#[test]
fn test_multiple_commands_undo_redo() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);
    let obj_ref1 = Ref::new(5, 0);
    let obj_ref2 = Ref::new(6, 0);

    let cmd1 = Box::new(ModifyCommand {
        obj_ref: obj_ref1,
        old_value: None,
        new_value: PDFObject::Number(10.0),
        had_old_value: false,
    });

    let cmd2 = Box::new(ModifyCommand {
        obj_ref: obj_ref2,
        old_value: None,
        new_value: PDFObject::Number(20.0),
        had_old_value: false,
    });

    history.execute(&mut delta, cmd1, None).unwrap();
    history.execute(&mut delta, cmd2, None).unwrap();

    // Both objects should be modified
    assert!(delta.get(&obj_ref1).is_some());
    assert!(delta.get(&obj_ref2).is_some());

    // Undo both
    history.undo(&mut delta).unwrap();
    history.undo(&mut delta).unwrap();

    assert!(!history.can_undo());
    assert!(history.can_redo());

    // Redo both
    history.redo(&mut delta).unwrap();
    history.redo(&mut delta).unwrap();

    assert!(history.can_undo());
    assert!(!history.can_redo());

    // Both objects should be modified again
    assert!(delta.get(&obj_ref1).is_some());
    assert!(delta.get(&obj_ref2).is_some());
}

#[test]
fn test_clear_resets_state() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);

    // Make some changes
    delta.modify_object(Ref::new(1, 0), PDFObject::Number(1.0));
    delta.add_object(PDFObject::Number(2.0));
    delta.delete_object(Ref::new(3, 0));

    assert_eq!(delta.change_count(), 3);

    // Clear document state and the separate local command history.
    delta.clear();
    history.clear();

    assert_eq!(delta.change_count(), 0);
    assert!(!history.can_undo());
    assert!(!history.can_redo());
    assert!(delta.get(&Ref::new(1, 0)).is_none());
    assert!(!delta.is_deleted(&Ref::new(3, 0)));
}

#[test]
fn test_undo_when_empty_returns_error() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);

    let result = history.undo(&mut delta);
    assert!(result.is_err());
}

#[test]
fn test_redo_when_empty_returns_error() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);

    let result = history.redo(&mut delta);
    assert!(result.is_err());
}

#[test]
fn test_new_command_clears_redo_stack() {
    let mut history = CommandHistory::new();
    let mut delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(5, 0);

    let cmd1 = Box::new(ModifyCommand {
        obj_ref,
        old_value: None,
        new_value: PDFObject::Number(10.0),
        had_old_value: false,
    });

    let cmd2 = Box::new(ModifyCommand {
        obj_ref,
        old_value: None,
        new_value: PDFObject::Number(20.0),
        had_old_value: false,
    });

    history.execute(&mut delta, cmd1, None).unwrap();
    history.undo(&mut delta).unwrap();
    assert!(history.can_redo());

    // New command should clear redo stack
    history.execute(&mut delta, cmd2, None).unwrap();
    assert!(!history.can_redo());
}

#[test]
fn test_delta_layer_returns_none_for_nonexistent_objects() {
    let delta = DeltaLayer::new(100);
    let obj_ref = Ref::new(999, 0);

    assert!(delta.get(&obj_ref).is_none());
    assert!(!delta.is_deleted(&obj_ref));
}
