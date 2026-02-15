//! PDF annotation parsing and extraction.
//!
//! This module handles parsing of PDF annotations (markup, links, form fields, etc.).
//!
//! Based on PDF.js src/core/annotation.js.

use crate::core::error::PDFResult;
use crate::core::parser::PDFObject;
use rustc_hash::FxHashMap;
use std::collections::HashSet;

/// Annotation types in PDF documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationType {
    /// Text annotation (sticky note, comment)
    Text,

    /// Link annotation (hyperlink, internal link)
    Link,

    /// Free text annotation (typewriter text)
    FreeText,

    /// Line annotation
    Line,

    /// Square annotation
    Square,

    /// Circle annotation
    Circle,

    /// Polygon annotation
    Polygon,

    /// Polyline annotation
    PolyLine,

    /// Highlight annotation
    Highlight,

    /// Underline annotation
    Underline,

    /// Squiggly underline annotation
    Squiggly,

    /// Strikeout annotation
    StrikeOut,

    /// Stamp annotation
    Stamp,

    /// Caret annotation
    Caret,

    /// Ink annotation
    Ink,

    /// Popup annotation
    Popup,

    /// File attachment annotation
    FileAttachment,

    /// Sound annotation
    Sound,

    /// Movie annotation
    Movie,

    /// Widget annotation (form field)
    Widget,

    /// Screen annotation
    Screen,

    /// PrinterMark annotation
    PrinterMark,

    /// TrapNet annotation
    TrapNet,

    /// Watermark annotation
    Watermark,

    /// 3D annotation
    Model3D,

    /// Redact annotation
    Redact,

    /// Unknown annotation type
    Unknown(String),
}

impl AnnotationType {
    /// Parse annotation type from a name object.
    pub fn from_name(name: &str) -> Self {
        match name {
            "Text" => AnnotationType::Text,
            "Link" => AnnotationType::Link,
            "FreeText" => AnnotationType::FreeText,
            "Line" => AnnotationType::Line,
            "Square" => AnnotationType::Square,
            "Circle" => AnnotationType::Circle,
            "Polygon" => AnnotationType::Polygon,
            "PolyLine" => AnnotationType::PolyLine,
            "Highlight" => AnnotationType::Highlight,
            "Underline" => AnnotationType::Underline,
            "Squiggly" => AnnotationType::Squiggly,
            "StrikeOut" => AnnotationType::StrikeOut,
            "Stamp" => AnnotationType::Stamp,
            "Caret" => AnnotationType::Caret,
            "Ink" => AnnotationType::Ink,
            "Popup" => AnnotationType::Popup,
            "FileAttachment" => AnnotationType::FileAttachment,
            "Sound" => AnnotationType::Sound,
            "Movie" => AnnotationType::Movie,
            "Widget" => AnnotationType::Widget,
            "Screen" => AnnotationType::Screen,
            "PrinterMark" => AnnotationType::PrinterMark,
            "TrapNet" => AnnotationType::TrapNet,
            "Watermark" => AnnotationType::Watermark,
            "3D" => AnnotationType::Model3D,
            "Redact" => AnnotationType::Redact,
            other => AnnotationType::Unknown(other.to_string()),
        }
    }
}

/// Annotation flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnnotationFlags {
    /// Invisible (if set, don't display)
    pub invisible: bool,

    /// Hidden (if set, don't display or print)
    pub hidden: bool,

    /// Print (if set, print annotation)
    pub print: bool,

    /// No zoom (if set, don't scale annotation)
    pub no_zoom: bool,

    /// No rotate (if set, don't rotate annotation)
    pub no_rotate: bool,

    /// No view (if set, don't display on screen)
    pub no_view: bool,

    /// Read only (if set, don't allow interaction)
    pub read_only: bool,

    /// Locked (if set, don't allow content to be modified)
    pub locked: bool,

    /// Toggle no view (if set, invert view state)
    pub toggle_no_view: bool,

    /// Locked contents (if set, don't allow content to be modified)
    pub locked_contents: bool,
}

impl AnnotationFlags {
    /// Parse annotation flags from an integer.
    pub fn from_flags(flags: i32) -> Self {
        AnnotationFlags {
            invisible: (flags & 1) != 0,
            hidden: (flags & 2) != 0,
            print: (flags & 4) != 0,
            no_zoom: (flags & 8) != 0,
            no_rotate: (flags & 16) != 0,
            no_view: (flags & 32) != 0,
            read_only: (flags & 64) != 0,
            locked: (flags & 128) != 0,
            toggle_no_view: (flags & 256) != 0,
            locked_contents: (flags & 512) != 0,
        }
    }
}

/// A rectangle defining the annotation's location on the page.
pub type AnnotationRect = [f64; 4]; // [llx, lly, urx, ury]

/// Border style for annotations.
#[derive(Debug, Clone)]
pub struct AnnotationBorder {
    /// Horizontal corner radius
    pub horizontal_corner_radius: f64,

    /// Vertical corner radius
    pub vertical_corner_radius: f64,

    /// Border width
    pub width: f64,

    /// Border style (Solid, Dashed, etc.)
    pub style: Option<String>,
}

/// Color for annotations (RGB or CMYK).
pub type AnnotationColor = Vec<f64>;

/// An annotation on a PDF page.
#[derive(Debug, Clone)]
pub struct Annotation {
    /// The annotation type
    pub annotation_type: AnnotationType,

    /// The annotation rectangle (location on page)
    pub rect: AnnotationRect,

    /// Annotation contents (text for notes, etc.)
    pub contents: Option<String>,

    /// Annotation flags (visibility, etc.)
    pub flags: AnnotationFlags,

    /// Border style
    pub border: Option<AnnotationBorder>,

    /// Color (RGB or CMYK)
    pub color: Option<AnnotationColor>,

    /// Modification date
    pub modification_date: Option<String>,

    /// The appearance dictionary
    pub appearance: Option<PDFObject>,

    /// Annotation-specific data
    pub data: AnnotationData,
}

/// Annotation-specific data.
#[derive(Debug, Clone)]
pub enum AnnotationData {
    /// No additional data
    None,

    /// Link annotation data
    Link(LinkAnnotation),

    /// Text annotation data
    Text(TextAnnotation),

    /// Widget annotation data (form field)
    Widget(WidgetAnnotation),

    /// File attachment annotation data
    FileAttachment(FileAttachmentAnnotation),

    /// Popup annotation data
    Popup(PopupAnnotation),
}

impl Default for AnnotationData {
    fn default() -> Self {
        AnnotationData::None
    }
}

/// Link annotation data.
#[derive(Debug, Clone)]
pub struct LinkAnnotation {
    /// The link action
    pub action: LinkAction,
}

/// Actions that can be performed by a link.
#[derive(Debug, Clone)]
pub enum LinkAction {
    /// Go to a destination in the same document
    GoTo {
        /// Page index (0-based)
        page_index: usize,
        /// Destination type and parameters
        dest: crate::core::outline::DestinationType,
    },

    /// Go to a named destination
    GoToNamed {
        /// Named destination string
        name: String,
    },

    /// URI action (web link)
    URI {
        /// The URL
        url: String,
        /// Whether to open in new window
        is_map: bool,
    },

    /// Launch action (launch an application)
    Launch {
        /// Application to launch
        application: String,
        /// Parameters for the application
        parameters: Option<String>,
    },

    /// Go to remote PDF document
    GoToRemote {
        /// File specification
        file_spec: String,
        /// Destination in remote document
        dest: Option<String>,
        /// Whether to open in new window
        new_window: bool,
    },

    /// Named action
    Named {
        /// Action name (e.g., NextPage, PrevPage, FirstPage, LastPage)
        name: String,
    },

    /// Unknown action type
    Unknown,
}

/// Text annotation data (sticky notes, comments).
#[derive(Debug, Clone)]
pub struct TextAnnotation {
    /// Whether the annotation is open by default
    pub open: bool,

    /// The name of the icon (e.g., "Comment", "Note", "Help")
    pub icon: Option<String>,

    /// The state of the annotation (e.g., "Accepted", "Rejected")
    pub state: Option<String>,

    /// The state model
    pub state_model: Option<String>,
}

/// Widget annotation data (form field).
#[derive(Debug, Clone)]
pub struct WidgetAnnotation {
    /// The form field type
    pub field_type: FormFieldType,

    /// The field name
    pub field_name: Option<String>,

    /// The field value
    pub field_value: Option<String>,

    /// The default value
    pub default_value: Option<String>,

    /// Export value (for checkboxes/radio buttons)
    pub export_value: Option<String>,
}

/// Form field types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormFieldType {
    /// Button field (push button, checkbox, radio button)
    Button,

    /// Text field
    Text,

    /// Choice field (list box, combo box)
    Choice,

    /// Signature field
    Signature,

    /// Unknown field type
    Unknown(String),
}

/// File attachment annotation data.
#[derive(Debug, Clone)]
pub struct FileAttachmentAnnotation {
    /// The file specification
    pub file_spec: String,

    /// The file name
    pub file_name: Option<String>,

    /// Description of the file
    pub description: Option<String>,
}

/// Popup annotation data.
#[derive(Debug, Clone)]
pub struct PopupAnnotation {
    /// Whether the popup is open by default
    pub open: bool,

    /// The parent annotation reference
    pub parent_ref: Option<(u32, u32)>,
}

/// Parses annotations from a page's Annots array.
///
/// # Arguments
/// * `annots_obj` - The Annots object (array or reference to array)
/// * `xref` - The cross-reference table for resolving references
///
/// # Returns
/// A vector of parsed annotations
pub fn parse_annotations(
    annots_obj: &PDFObject,
    xref: &mut crate::core::XRef,
) -> PDFResult<Vec<Annotation>> {
    // Resolve the Annots array
    let annots_array = match annots_obj {
        PDFObject::Array(arr) => arr.clone(),
        PDFObject::Ref(ref_obj) => {
            let fetched = xref.fetch(ref_obj.num, ref_obj.generation)?;
            match &*fetched {
                PDFObject::Array(arr) => arr.clone(),
                _ => return Ok(Vec::new()),
            }
        }
        _ => return Ok(Vec::new()),
    };

    let mut annotations = Vec::new();
    let mut visited_refs: HashSet<(u32, u32)> = HashSet::new();

    for annot_ref in annots_array.iter() {
        let annot_dict = match &**annot_ref {
            PDFObject::Ref(ref_obj) => {
                let ref_key = (ref_obj.num, ref_obj.generation);

                // Prevent circular references
                if visited_refs.contains(&ref_key) {
                    continue;
                }
                visited_refs.insert(ref_key);

                let fetched = xref.fetch(ref_obj.num, ref_obj.generation)?;
                match &*fetched {
                    PDFObject::Dictionary(dict) => dict.clone(),
                    _ => continue,
                }
            }
            PDFObject::Dictionary(dict) => dict.clone(),
            _ => continue,
        };

        // Parse the annotation
        // We need to convert the HashMap to use FxHasher
        let mut fx_dict = FxHashMap::default();
        for (k, v) in annot_dict.iter() {
            fx_dict.insert(k.clone(), v.clone());
        }

        if let Ok(annot) = parse_annotation_dict(&fx_dict, xref) {
            annotations.push(annot);
        } else {
            // Silently skip invalid annotations
        }
    }

    Ok(annotations)
}

/// Parses a single annotation dictionary.
fn parse_annotation_dict(
    dict: &FxHashMap<String, PDFObject>,
    xref: &mut crate::core::XRef,
) -> PDFResult<Annotation> {
    // Get the annotation subtype (type)
    let subtype = dict.get("Subtype");
    let annotation_type = match subtype {
        Some(PDFObject::Name(name)) => AnnotationType::from_name(name),
        _ => AnnotationType::Unknown("".to_string()),
    };

    // Get the rectangle
    let rect = match dict.get("Rect") {
        Some(PDFObject::Array(arr)) if arr.len() >= 4 => {
            let mut result = [0.0; 4];
            for (i, val) in arr.iter().take(4).enumerate() {
                match &**val {
                    PDFObject::Number(n) => result[i] = *n,
                    _ => result[i] = 0.0,
                }
            }
            result
        }
        _ => [0.0, 0.0, 0.0, 0.0],
    };

    // Get the contents
    let contents = match dict.get("Contents") {
        Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
            Some(String::from_utf8_lossy(bytes).to_string())
        }
        _ => None,
    };

    // Get the flags
    let flags = match dict.get("F") {
        Some(PDFObject::Number(n)) => AnnotationFlags::from_flags(*n as i32),
        _ => AnnotationFlags::default(),
    };

    // Get the color
    let color = match dict.get("C") {
        Some(PDFObject::Array(arr)) => {
            let mut result = Vec::new();
            for val in arr.iter() {
                match &**val {
                    PDFObject::Number(n) => result.push(*n),
                    _ => result.push(0.0),
                }
            }
            Some(result)
        }
        _ => None,
    };

    // Get the modification date
    let modification_date = match dict.get("M") {
        Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
            Some(String::from_utf8_lossy(bytes).to_string())
        }
        _ => None,
    };

    // Get the appearance
    let appearance = dict.get("AP").cloned();

    // Parse annotation-specific data
    let data = parse_annotation_data(&annotation_type, dict, xref)?;

    Ok(Annotation {
        annotation_type,
        rect,
        contents,
        flags,
        border: None, // TODO: Parse border
        color,
        modification_date,
        appearance,
        data,
    })
}

/// Parses annotation-specific data based on the annotation type.
fn parse_annotation_data(
    annotation_type: &AnnotationType,
    dict: &FxHashMap<String, PDFObject>,
    xref: &mut crate::core::XRef,
) -> PDFResult<AnnotationData> {
    match annotation_type {
        AnnotationType::Link => {
            let action = parse_link_action(dict, xref)?;
            Ok(AnnotationData::Link(LinkAnnotation { action }))
        }
        AnnotationType::Text => {
            let open = match dict.get("Open") {
                Some(PDFObject::Boolean(b)) => *b,
                _ => false,
            };
            let icon = match dict.get("Name") {
                Some(PDFObject::Name(name)) => Some(name.clone()),
                _ => None,
            };
            let state = match dict.get("State") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                _ => None,
            };
            let state_model = match dict.get("StateModel") {
                Some(PDFObject::Name(name)) => Some(name.clone()),
                _ => None,
            };

            Ok(AnnotationData::Text(TextAnnotation {
                open,
                icon,
                state,
                state_model,
            }))
        }
        AnnotationType::Widget => {
            let field_type = match dict.get("FT") {
                Some(PDFObject::Name(name)) => match name.as_str() {
                    "Btn" => FormFieldType::Button,
                    "Tx" => FormFieldType::Text,
                    "Ch" => FormFieldType::Choice,
                    "Sig" => FormFieldType::Signature,
                    _ => FormFieldType::Unknown(name.clone()),
                },
                _ => FormFieldType::Unknown("".to_string()),
            };

            let field_name = match dict.get("T") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                _ => None,
            };

            let field_value = match dict.get("V") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                Some(PDFObject::Name(name)) => Some(name.clone()),
                _ => None,
            };

            let default_value = match dict.get("DV") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                _ => None,
            };

            let export_value = match dict.get("ExportValue") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                _ => None,
            };

            Ok(AnnotationData::Widget(WidgetAnnotation {
                field_type,
                field_name,
                field_value,
                default_value,
                export_value,
            }))
        }
        AnnotationType::FileAttachment => {
            let file_spec = match dict.get("FS") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    String::from_utf8_lossy(bytes).to_string()
                }
                _ => String::new(),
            };

            let file_name = match dict.get("F") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                _ => None,
            };

            let description = match dict.get("Desc") {
                Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).to_string())
                }
                _ => None,
            };

            Ok(AnnotationData::FileAttachment(FileAttachmentAnnotation {
                file_spec,
                file_name,
                description,
            }))
        }
        AnnotationType::Popup => {
            let open = match dict.get("Open") {
                Some(PDFObject::Boolean(b)) => *b,
                _ => false,
            };

            let parent_ref = match dict.get("Parent") {
                Some(PDFObject::Ref(ref_obj)) => Some((ref_obj.num, ref_obj.generation)),
                _ => None,
            };

            Ok(AnnotationData::Popup(PopupAnnotation { open, parent_ref }))
        }
        _ => Ok(AnnotationData::None),
    }
}

/// Parses the action for a link annotation.
fn parse_link_action(
    dict: &FxHashMap<String, PDFObject>,
    xref: &mut crate::core::XRef,
) -> PDFResult<LinkAction> {
    // Check for action dictionary (/A)
    if let Some(action_obj) = dict.get("A") {
        let action_dict = match xref.fetch_if_ref(action_obj)? {
            PDFObject::Dictionary(dict) => dict,
            _ => return Ok(LinkAction::Unknown),
        };

        let action_type = match action_dict.get("S") {
            Some(PDFObject::Name(name)) => name.as_str(),
            _ => return Ok(LinkAction::Unknown),
        };

        match action_type {
            "URI" => {
                let uri = match action_dict.get("URI") {
                    Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                        String::from_utf8_lossy(bytes).to_string()
                    }
                    _ => String::new(),
                };
                let is_map = match action_dict.get("IsMap") {
                    Some(PDFObject::Boolean(b)) => *b,
                    _ => false,
                };
                return Ok(LinkAction::URI { url: uri, is_map });
            }
            "GoTo" => {
                if let Some(dest_obj) = action_dict.get("D") {
                    return parse_goto_destination(dest_obj, xref);
                }
            }
            "GoToR" => {
                let file_spec = match action_dict.get("F") {
                    Some(PDFObject::String(bytes)) => String::from_utf8_lossy(bytes).to_string(),
                    _ => String::new(),
                };
                let dest = match action_dict.get("D") {
                    Some(PDFObject::String(bytes)) | Some(PDFObject::HexString(bytes)) => {
                        Some(String::from_utf8_lossy(bytes).to_string())
                    }
                    _ => None,
                };
                let new_window = match action_dict.get("NewWindow") {
                    Some(PDFObject::Boolean(b)) => *b,
                    _ => false,
                };
                return Ok(LinkAction::GoToRemote {
                    file_spec,
                    dest,
                    new_window,
                });
            }
            "Launch" => {
                let application = match action_dict.get("F") {
                    Some(PDFObject::String(bytes)) => String::from_utf8_lossy(bytes).to_string(),
                    _ => String::new(),
                };
                let parameters = match action_dict.get("P") {
                    Some(PDFObject::String(bytes)) => {
                        Some(String::from_utf8_lossy(bytes).to_string())
                    }
                    _ => None,
                };
                return Ok(LinkAction::Launch {
                    application,
                    parameters,
                });
            }
            "Named" => {
                let name = match action_dict.get("N") {
                    Some(PDFObject::Name(name)) => name.clone(),
                    _ => String::new(),
                };
                return Ok(LinkAction::Named { name });
            }
            _ => {}
        }
    }

    // Check for destination (/Dest)
    if let Some(dest_obj) = dict.get("Dest") {
        return parse_goto_destination(dest_obj, xref);
    }

    Ok(LinkAction::Unknown)
}

/// Parses a GoTo destination.
fn parse_goto_destination(
    dest_obj: &PDFObject,
    xref: &mut crate::core::XRef,
) -> PDFResult<LinkAction> {
    match dest_obj {
        PDFObject::Array(arr) => {
            if arr.is_empty() {
                return Ok(LinkAction::Unknown);
            }

            // First element is the page reference
            let page_ref = &arr[0];

            // Resolve page reference to page index
            // For now, we'll use a placeholder since we don't have the document context here
            // In the full implementation, we'd need to pass the document to resolve this
            return Ok(LinkAction::GoToNamed {
                name: format!("{:?}", page_ref), // Placeholder
            });
        }
        PDFObject::String(bytes) | PDFObject::HexString(bytes) => {
            let name = String::from_utf8_lossy(bytes).to_string();
            Ok(LinkAction::GoToNamed { name })
        }
        PDFObject::Name(name) => Ok(LinkAction::GoToNamed { name: name.clone() }),
        _ => Ok(LinkAction::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_annotation_type_from_name() {
        assert_eq!(AnnotationType::from_name("Text"), AnnotationType::Text);
        assert_eq!(AnnotationType::from_name("Link"), AnnotationType::Link);
        assert_eq!(
            AnnotationType::from_name("Highlight"),
            AnnotationType::Highlight
        );
        assert_eq!(AnnotationType::from_name("Widget"), AnnotationType::Widget);
        assert_eq!(
            AnnotationType::from_name("UnknownType"),
            AnnotationType::Unknown("UnknownType".to_string())
        );
    }

    #[test]
    fn test_annotation_flags() {
        let flags = AnnotationFlags::from_flags(0b101);
        assert!(flags.invisible);
        assert!(flags.print);
        assert!(!flags.hidden);
    }

    #[test]
    fn test_form_field_type() {
        assert_eq!(FormFieldType::Button, FormFieldType::Button);
        assert_eq!(FormFieldType::Text, FormFieldType::Text);
    }
}
