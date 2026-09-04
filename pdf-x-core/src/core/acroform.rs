//! AcroForm field tree (ISO 32000-1 §12.7).
//!
//! The field tree is a hierarchy of field dictionaries rooted at the
//! catalog's `/AcroForm /Fields`. A field dictionary may combine with a
//! widget annotation (merged field+widget), carry `/Kids` that are either
//! child fields (have `/T`) or pure widget annotations (no `/T`, e.g.
//! radio-button members sharing the parent's name), or both.
//!
//! Reference: pdf.js `Annotation._constructFieldName` (annotation.js:1458)
//! for the fully-qualified-name walk (ancestor `/T` names joined with "." and
//! a visited-set cycle guard), and `getInheritableProperty`
//! (core_utils.js:154) for the inheritable `/FT` and `/V` entries.

use std::collections::HashSet;

use crate::core::annotation::FormFieldType;
use crate::core::error::PDFResult;
use crate::core::page::PageResolver;
use crate::core::parser::{PDFObject, Ref};

/// One node of the AcroForm field tree.
#[derive(Debug, Clone)]
pub struct FormField {
    /// The field dictionary's object reference. `None` for inline dictionaries.
    pub field_ref: Option<Ref>,

    /// Fully qualified name: the ancestor `/T` names joined with "." —
    /// pdf.js `_constructFieldName`. Empty string when no `/T` exists on the
    /// path (nameless fields, tolerated like pdf.js does).
    pub fully_qualified_name: String,

    /// `/FT` resolved with inheritance — the nearest dictionary on the
    /// parent chain that carries `/FT` wins (pdf.js `getInheritableProperty`).
    pub field_type: FormFieldType,

    /// `true` when the field has no child fields (only widgets or nothing) —
    /// terminal fields are the ones that carry values.
    pub terminal: bool,

    /// `/V` resolved with inheritance, as a display string (text values and
    /// checkbox on-states like `/Yes`). Non-text values (arrays, streams,
    /// dicts) are reported as `None` — Choice fields are out of scope for v1.
    pub value: Option<String>,

    /// Widget annotations belonging to this field: the field dict itself when
    /// it is a merged field+widget (`/Subtype /Widget`), plus `/Kids` entries
    /// without `/T` (pure widgets, e.g. radio-button members).
    pub widget_refs: Vec<Ref>,

    /// Child field dictionaries (non-terminal fields).
    pub kids: Vec<FormField>,
}

/// `/FT` and `/V` values a subtree inherits from its ancestors (spec
/// §12.7.3.1: field entries are inherited by terminal fields).
#[derive(Clone, Copy)]
struct Inherited<'a> {
    ft: Option<&'a str>,
    v: Option<&'a PDFObject>,
}

/// Finds a field by exact fully qualified name, depth-first. Field names are
/// matched verbatim — dotted names are the join produced by
/// `_constructFieldName`, not a path to walk.
pub fn find_by_name<'a>(fields: &'a [FormField], name: &str) -> Option<&'a FormField> {
    for field in fields {
        if field.fully_qualified_name == name {
            return Some(field);
        }
        if let Some(found) = find_by_name(&field.kids, name) {
            return Some(found);
        }
    }
    None
}

/// Collects every fully qualified name in the tree (for error messages).
pub fn collect_names(fields: &[FormField], out: &mut Vec<String>) {
    for field in fields {
        if !field.fully_qualified_name.is_empty() {
            out.push(field.fully_qualified_name.clone());
        }
        collect_names(&field.kids, out);
    }
}

/// Maps a `/FT` name to `FormFieldType` — same mapping as the widget parsing
/// in `annotation.rs` (`"Btn" => Button`, …).
fn field_type_from_name(name: &str) -> FormFieldType {
    match name {
        "Btn" => FormFieldType::Button,
        "Tx" => FormFieldType::Text,
        "Ch" => FormFieldType::Choice,
        "Sig" => FormFieldType::Signature,
        other => FormFieldType::Unknown(other.to_string()),
    }
}

/// The reference behind `obj`, if it is an indirect one.
fn ref_of(obj: &PDFObject) -> Option<Ref> {
    match obj {
        PDFObject::Ref(r) => Some(Ref::new(r.num, r.generation)),
        _ => None,
    }
}

/// Reads a `/T` partial name as a lossy UTF-8 string.
fn partial_name(dict: &PDFObject) -> Option<String> {
    let dict = dict.as_dictionary()?;
    match dict.get("T") {
        Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
            Some(String::from_utf8_lossy(bytes).to_string())
        }
        _ => None,
    }
}

/// Renders `/V` as a display string. Text values keep their bytes; name
/// values (checkbox on-states, e.g. `/Yes`) keep the name text. Everything
/// else (arrays, streams, dicts — Choice/radio-group values) is `None`.
fn value_as_string(value: &PDFObject) -> Option<String> {
    match value {
        PDFObject::String(bytes) | PDFObject::HexString(bytes) => {
            Some(String::from_utf8_lossy(bytes).to_string())
        }
        PDFObject::Name(name) => Some(name.clone()),
        _ => None,
    }
}

/// Walks the AcroForm field tree depth-first, in `/Fields` order.
///
/// All object resolution goes through the resolver (delta-aware
/// `DocStore` in production). A visited set guards against reference
/// cycles — the same guard pdf.js uses in `_constructFieldName`.
pub(crate) fn field_tree<R: PageResolver>(
    acroform: &PDFObject,
    resolver: &mut R,
) -> PDFResult<Vec<FormField>> {
    let Some(acroform_dict) = acroform.as_dictionary() else {
        return Ok(Vec::new());
    };
    let Some(fields_obj) = acroform_dict.get("Fields") else {
        return Ok(Vec::new());
    };
    let fields = resolver.resolve_if_ref(fields_obj)?;
    let Some(entries) = fields.as_array() else {
        return Ok(Vec::new());
    };

    let mut visited = HashSet::new();
    let mut out = Vec::new();
    for entry in entries {
        build_field(
            entry,
            ref_of(entry),
            &[],
            Inherited { ft: None, v: None },
            &mut visited,
            resolver,
            &mut out,
        )?;
    }
    Ok(out)
}

/// Parses one field dictionary (and its subtree) and appends it to `out`.
fn build_field<R: PageResolver>(
    entry: &PDFObject,
    entry_ref: Option<Ref>,
    prefix: &[String],
    inherited: Inherited<'_>,
    visited: &mut HashSet<(u32, u32)>,
    resolver: &mut R,
    out: &mut Vec<FormField>,
) -> PDFResult<()> {
    let resolved = resolver.resolve_if_ref(entry)?;
    let Some(dict) = resolved.as_dictionary() else {
        // Broken generators may put nulls in /Fields — skip non-dictionaries.
        return Ok(());
    };

    // Cycle guard on the object identity (inline dicts have none; they cannot
    // form reference cycles).
    if let Some(r) = entry_ref {
        if !visited.insert((r.num, r.generation)) {
            return Ok(());
        }
    }

    let own_name = partial_name(&resolved);
    let mut fqn_parts: Vec<&str> = prefix.iter().map(String::as_str).collect();
    if let Some(name) = &own_name {
        fqn_parts.push(name);
    }
    let fully_qualified_name = fqn_parts.join(".");

    // Nearest dictionary on the parent chain wins for both entries.
    let field_type = match dict.get("FT") {
        Some(PDFObject::Name(name)) => field_type_from_name(name),
        _ => inherited
            .ft
            .map_or(FormFieldType::Unknown(String::new()), field_type_from_name),
    };
    let effective = Inherited {
        ft: match dict.get("FT") {
            Some(PDFObject::Name(name)) => Some(name.as_str()),
            _ => inherited.ft,
        },
        v: match dict.get("V") {
            Some(v) => Some(v),
            _ => inherited.v,
        },
    };
    let value = dict
        .get("V")
        .and_then(value_as_string)
        .or_else(|| effective.v.and_then(value_as_string));

    // Partition /Kids into child fields (have /T) and pure widgets (no /T).
    let mut widget_refs = Vec::new();
    let mut kids = Vec::new();
    if let Some(kids_obj) = dict.get("Kids") {
        let kids_resolved = resolver.resolve_if_ref(kids_obj)?;
        if let Some(kid_entries) = kids_resolved.as_array() {
            for kid_entry in kid_entries {
                let kid = resolver.resolve_if_ref(kid_entry)?;
                let Some(kid_dict) = kid.as_dictionary() else {
                    continue;
                };
                if kid_dict.get("T").is_some() {
                    let mut child_prefix = prefix.to_vec();
                    if let Some(name) = &own_name {
                        child_prefix.push(name.clone());
                    }
                    let mut child_out = Vec::new();
                    build_field(
                        kid_entry,
                        ref_of(kid_entry),
                        &child_prefix,
                        effective,
                        visited,
                        resolver,
                        &mut child_out,
                    )?;
                    kids.extend(child_out);
                } else {
                    // Widget members of merged collections (radio groups)
                    // have no /T of their own — they belong to this field's
                    // widget set. Not recursed into, so no cycle risk.
                    if let Some(r) = ref_of(kid_entry) {
                        widget_refs.push(r);
                    }
                }
            }
        }
    }

    // A merged field+widget (the common terminal shape) contributes its own ref.
    let is_widget = dict
        .get("Subtype")
        .and_then(PDFObject::as_name)
        .is_some_and(|name| name == "Widget");
    if is_widget {
        if let Some(r) = entry_ref {
            widget_refs.insert(0, r);
        }
    }

    out.push(FormField {
        field_ref: entry_ref,
        fully_qualified_name,
        field_type,
        terminal: kids.is_empty(),
        value,
        widget_refs,
        kids,
    });
    Ok(())
}
