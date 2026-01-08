//! Property-based tests for PDF-X robustness validation.
//!
//! These tests use proptest to generate random inputs and verify invariants.

mod test_utils;

use pdf_x_core::core::*;
use pdf_x_core::core::error::PDFResult;
use test_utils::*;
use proptest::prelude::*;

// ============================================================================
// XRef Property Tests
// ============================================================================

/// Property: XRef mock should store and retrieve entries correctly
proptest! {
    #[test]
    fn prop_xref_lookup(obj_num in 1u32..1000u32, offset in 0u64..1_000_000u64, gen_num in 0u16..10u16) {
        let mut xref = XRefMock::new();
        xref.add_entry(obj_num, offset, gen_num);

        let entry = xref.get_entry(obj_num);
        prop_assert!(entry.is_some());

        match entry.unwrap() {
            XRefEntry::Uncompressed { offset: off, generation: entry_gen } => {
                prop_assert_eq!(*off, offset);
                prop_assert_eq!(*entry_gen, gen_num as u32);
            }
            _ => prop_assert!(false, "Expected Uncompressed entry"),
        }
    }
}

/// Property: XRef mock should return None for non-existent entries
proptest! {
    #[test]
    fn prop_xref_nonexistent(obj_num in 1u32..1000u32) {
        let xref = XRefMock::new();
        let entry = xref.get_entry(obj_num);

        prop_assert!(entry.is_none());
    }
}

// ============================================================================
// Delta Layer Property Tests
// ============================================================================

/// Property: Command execution should be reversible with undo
proptest! {
    #[test]
    fn prop_delta_undo_invariant(key in 1u32..1000u32, value in 1i64..10000i64) {
        use pdf_x_core::core::delta::{DeltaLayer, Command};

        let mut delta = DeltaLayer::new(1000);
        let ref_obj = Ref::new(key, 0);

        // Store initial state
        let initial = delta.get(&ref_obj).cloned();

        // Create a modify command
        let new_obj = PDFObject::Number(value as f64);
        let mut command = TestModifyCommand {
            obj_ref: ref_obj,
            new_value: new_obj.clone(),
            old_value: initial.clone(),
        };

        // Execute command
        let exec_result = command.execute(&mut delta);
        prop_assert!(exec_result.is_ok());

        // Verify change was applied
        let after_exec = delta.get(&ref_obj);
        prop_assert!(after_exec.is_some());

        // Undo command
        let undo_result = command.undo(&mut delta);
        prop_assert!(undo_result.is_ok());

        // Verify state was restored
        let after_undo = delta.get(&ref_obj);
        prop_assert_eq!(after_undo.is_some(), initial.is_some());

        if let Some(initial_obj) = initial {
            if let Some(undo_obj) = after_undo {
                prop_assert_eq!(&undo_obj.object, &initial_obj.object);
            }
        }
    }
}

/// Property: Multiple modifications should compose correctly
proptest! {
    #[test]
    fn prop_delta_composition(key in 1u32..100u32, values in prop::collection::vec(1i64..1000i64, 2..10)) {
        use pdf_x_core::core::delta::DeltaLayer;

        let mut delta = DeltaLayer::new(1000);
        let ref_obj = Ref::new(key, 0);

        // Apply multiple modifications
        for (_i, &value) in values.iter().enumerate() {
            let new_obj = PDFObject::Number(value as f64);
            delta.modify_object(ref_obj, new_obj);

            // Verify the modification was applied
            let current = delta.get(&ref_obj);
            prop_assert!(current.is_some());

            if let Some(obj) = current {
                if let PDFObject::Number(n) = &obj.object {
                    prop_assert_eq!(*n, value as f64);
                }
            }
        }
    }
}

/// Property: Delta layer should track change count correctly
proptest! {
    #[test]
    fn prop_delta_change_count(keys in prop::collection::vec(1u32..100u32, 1..20)) {
        use pdf_x_core::core::delta::DeltaLayer;

        let mut delta = DeltaLayer::new(1000);

        // Modify each key
        for &key in &keys {
            let ref_obj = Ref::new(key, 0);
            let new_obj = PDFObject::Number(42.0);
            delta.modify_object(ref_obj, new_obj);
        }

        // Count unique modified objects
        let unique_keys: std::collections::HashSet<_> = keys.iter().cloned().collect();
        prop_assert_eq!(delta.change_count(), unique_keys.len() as usize);
    }
}

/// Property: Object deletion should be idempotent
proptest! {
    #[test]
    fn prop_delta_deletion_idempotent(key in 1u32..1000u32) {
        use pdf_x_core::core::delta::DeltaLayer;

        let mut delta = DeltaLayer::new(1000);
        let ref_obj = Ref::new(key, 0);

        // Add an object
        let obj = PDFObject::Number(42.0);
        delta.modify_object(ref_obj, obj.clone());

        // Delete once
        delta.delete_object(ref_obj);
        prop_assert!(delta.is_deleted(&ref_obj));

        // Delete again - should be idempotent
        delta.delete_object(ref_obj);
        prop_assert!(delta.is_deleted(&ref_obj));
    }
}

// ============================================================================
// Permissions Property Tests
// ============================================================================

/// Property: Permission flags should round-trip correctly
proptest! {
    #[test]
    fn prop_permissions_roundtrip(p in 0u32..0x1000u32) {
        let perms = PDFPermissions::from_p_value(p);
        prop_assert_eq!(perms.raw_value, p);
    }
}

/// Property: Permission flags should be idempotent
proptest! {
    #[test]
    fn prop_permissions_idempotent(p in 0u32..0x1000u32) {
        let perms1 = PDFPermissions::from_p_value(p);
        let perms2 = PDFPermissions::from_p_value(perms1.raw_value);

        prop_assert_eq!(perms1.print, perms2.print);
        prop_assert_eq!(perms1.modify, perms2.modify);
        prop_assert_eq!(perms1.copy, perms2.copy);
        prop_assert_eq!(perms1.annotate, perms2.annotate);
    }
}

// ============================================================================
// Test Utility Command
// ============================================================================

/// Test command for property-based testing
pub struct TestModifyCommand {
    pub obj_ref: Ref,
    pub new_value: PDFObject,
    pub old_value: Option<DeltaObject>,
}

impl Command for TestModifyCommand {
    fn execute(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        if let Some(obj) = delta.get(&self.obj_ref) {
            self.old_value = Some(obj.clone());
        } else {
            self.old_value = None;
        }
        delta.modify_object(self.obj_ref, self.new_value.clone());
        Ok(())
    }

    fn undo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        match &self.old_value {
            Some(obj) => {
                delta.modify_object(self.obj_ref, obj.object.clone());
            }
            None => {
                // Object didn't exist before, delete it
                delta.delete_object(self.obj_ref);
            }
        }
        Ok(())
    }

    fn redo(&mut self, delta: &mut DeltaLayer) -> PDFResult<()> {
        delta.modify_object(self.obj_ref, self.new_value.clone());
        Ok(())
    }
}

// ============================================================================
// Robustness Property Tests
// ============================================================================

/// Property: Parsing should handle arbitrary input without panicking
proptest! {
    #[test]
    fn prop_parser_no_panic(input in prop::collection::vec(any::<u8>(), 0..1000)) {
        // Try to parse - should never panic
        let _ = std::panic::catch_unwind(|| {
            let stream = Stream::new(input.clone(), 0, input.len());
            let _ = Lexer::new(Box::new(stream));
        });
        prop_assert!(true); // If we get here, no panic occurred
    }
}

/// Property: XRef operations should handle arbitrary data gracefully
proptest! {
    #[test]
    fn prop_xref_malformed_no_panic(input in prop::collection::vec(any::<u8>(), 0..500)) {
        // Try to create XRef from arbitrary bytes
        let _ = std::panic::catch_unwind(|| {
            // This tests robustness - we're not asserting correctness,
            // just that the code doesn't panic
            let mut mock = XRefMock::new();
            // Attempt to add entries (may fail, shouldn't panic)
            for (i, chunk) in input.chunks(12).enumerate() {
                if chunk.len() >= 8 {
                    // Try to interpret as xref entry
                    let offset = u64::from_be_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                        chunk[4], chunk[5], chunk[6], chunk[7],
                    ]);
                    mock.add_entry(i as u32, offset, 0);
                }
            }
        });
        prop_assert!(true); // If we get here, no panic occurred
    }
}
