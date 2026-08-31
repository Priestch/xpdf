//! JSON output utilities.

use pdf_x_core::PDFObject;
use serde_json::Value;

/// Build a JSON envelope around command output.
pub fn envelope(command: &str, file: &str, data: Value) -> Value {
    serde_json::json!({
        "schema": 1,
        "command": command,
        "file": file,
        "data": data,
    })
}

/// Build a JSON error envelope.
pub fn error(command: &str, file: &str, msg: &str) -> Value {
    serde_json::json!({
        "schema": 1,
        "command": command,
        "file": file,
        "error": msg,
    })
}

/// Convert a PDFObject to a serde_json Value.
/// Dictionary keys are sorted for deterministic output.
pub fn pdf_object_to_value(obj: &PDFObject) -> Value {
    match obj {
        PDFObject::Null => Value::Null,
        PDFObject::Boolean(b) => Value::Bool(*b),
        PDFObject::Number(n) => {
            if n.fract() == 0.0 {
                Value::Number(serde_json::Number::from(*n as i64))
            } else {
                serde_json::json!(*n)
            }
        }
        PDFObject::String(s) => {
            serde_json::json!({"string": String::from_utf8_lossy(s)})
        }
        PDFObject::HexString(s) => {
            let hex: String = s.iter().map(|b| format!("{:02x}", b)).collect();
            serde_json::json!({"hex": hex})
        }
        PDFObject::InlineImageData(data) => {
            serde_json::json!({"inline_image": {"length": data.len()}})
        }
        PDFObject::Name(n) => serde_json::json!({"name": n}),
        PDFObject::Command(c) => serde_json::json!({"op": c}),
        PDFObject::Array(arr) => Value::Array(arr.iter().map(|v| pdf_object_to_value(v)).collect()),
        PDFObject::Dictionary(dict) => {
            let mut map = serde_json::Map::new();
            let mut keys: Vec<_> = dict.keys().collect();
            keys.sort();
            for key in keys {
                map.insert(key.clone(), pdf_object_to_value(&dict[key]));
            }
            Value::Object(map)
        }
        PDFObject::Stream { dict, data } => {
            serde_json::json!({
                "stream": {
                    "dict": pdf_object_to_value(&PDFObject::Dictionary(dict.clone())),
                    "data_length": data.len(),
                }
            })
        }
        PDFObject::Ref(r) => serde_json::json!({"ref": [r.num, r.generation]}),
        PDFObject::EOF => Value::String("EOF".into()),
    }
}
