use super::error::{PDFError, PDFResult};
use super::lexer::{Lexer, Token};
use std::collections::HashMap;

/// PDF object types as defined in the PDF specification.
///
/// This represents the complete set of PDF primitive objects that can appear
/// in a PDF file. Based on PDF.js's object model.
#[derive(Debug, Clone, PartialEq)]
pub enum PDFObject {
    /// Null value
    Null,

    /// Boolean value
    Boolean(bool),

    /// Numeric value (integers and reals)
    Number(f64),

    /// String value (from literal strings like (hello))
    String(Vec<u8>),

    /// Hex string value (from hex strings like <48656c6c6f>)
    HexString(Vec<u8>),

    /// Name value (from /Name)
    Name(String),

    /// Array of objects
    Array(Vec<PDFObject>),

    /// Dictionary (key-value pairs)
    Dictionary(HashMap<String, PDFObject>),

    /// Stream object (dictionary + binary data)
    Stream {
        dict: HashMap<String, PDFObject>,
        data: Vec<u8>,
    },

    /// Indirect object reference (like "5 0 R")
    Ref { num: u32, generation: u32 },

    /// End of file marker
    EOF,

    /// Command/operator (like 'q', 'Q', 'cm', 'Tj')
    /// Only used in content streams to distinguish operators from operands
    Command(String),
}

impl PDFObject {
    /// Returns true if this object is the EOF marker.
    pub fn is_eof(&self) -> bool {
        matches!(self, PDFObject::EOF)
    }

    /// Returns true if this object is null.
    pub fn is_null(&self) -> bool {
        matches!(self, PDFObject::Null)
    }

    /// Returns true if this object is a command/operator.
    pub fn is_command(&self, cmd: &str) -> bool {
        match self {
            PDFObject::Name(name) => name == cmd,
            PDFObject::Command(command) => command == cmd,
            _ => false,
        }
    }

    /// Returns true if this object looks like a command (operator).
    /// Used for content stream parsing to distinguish operators from operands.
    pub fn is_command_like(&self) -> bool {
        matches!(self, PDFObject::Command(_))
    }
}

/// PDF Parser for building PDF objects from tokens.
///
/// This is analogous to PDF.js's Parser class which converts tokens from the Lexer
/// into higher-level PDF objects (arrays, dictionaries, indirect references, etc.).
///
/// The parser maintains a 2-token lookahead buffer to enable detecting patterns
/// like indirect references (N1 N2 R) and stream objects (dictionary followed by "stream").
///
/// Based on PDF.js src/core/parser.js
pub struct Parser {
    /// The lexer that provides tokens
    lexer: Lexer,

    /// First lookahead token
    buf1: Option<Token>,

    /// Second lookahead token
    buf2: Option<Token>,

    /// Optional reference resolver for resolving indirect references during parsing
    /// This is needed when parsing streams with indirect /Length references
    ref_resolver: Option<Box<dyn Fn(u32, u32) -> PDFResult<PDFObject>>>,
}

impl Parser {
    /// Creates a new Parser from a Lexer.
    pub fn new(mut lexer: Lexer) -> PDFResult<Self> {
        // Fill the lookahead buffer
        let buf1 = Some(lexer.get_object()?);
        let buf2 = Some(lexer.get_object()?);

        Ok(Parser {
            lexer,
            buf1,
            buf2,
            ref_resolver: None,
        })
    }

    /// Sets a reference resolver function that can resolve indirect references.
    /// This is needed when parsing streams with indirect /Length references.
    pub fn set_ref_resolver<F>(&mut self, resolver: F)
    where
        F: Fn(u32, u32) -> PDFResult<PDFObject> + 'static,
    {
        self.ref_resolver = Some(Box::new(resolver));
    }

    /// Shifts the token buffer, advancing to the next token.
    ///
    /// This moves buf2 -> buf1 and reads a new token into buf2.
    fn shift(&mut self) -> PDFResult<()> {
        self.buf1 = self.buf2.take();
        self.buf2 = Some(self.lexer.get_object()?);
        Ok(())
    }

    /// Gets the next PDF object from the stream.
    ///
    /// This is the main parsing method that handles:
    /// - Arrays: [ obj1 obj2 ... ]
    /// - Dictionaries: << /Key1 value1 /Key2 value2 ... >>
    /// - Indirect references: N1 N2 R
    /// - Simple objects: numbers, strings, names, booleans, null
    ///
    /// Based on PDF.js Parser.getObj()
    pub fn get_object(&mut self) -> PDFResult<PDFObject> {
        let token = self.buf1.take().ok_or_else(|| {
            PDFError::Generic("Parser buffer is empty (this should not happen)".to_string())
        })?;

        self.shift()?;

        match token {
            // Array start: [ ... ]
            Token::ArrayStart => self.parse_array(),

            // Dictionary start: << ... >>
            Token::DictStart => self.parse_dictionary(),

            // Array/dict end tokens are errors here (should be consumed by parse_array/parse_dictionary)
            Token::ArrayEnd => Err(PDFError::Generic("Unexpected array end token".to_string())),
            Token::DictEnd => {
                Err(PDFError::Generic("Unexpected dictionary end token".to_string()))
            }

            // Number: could be the start of an indirect reference (N1 N2 R)
            Token::Number(n) => {
                // Check if this is an indirect reference: N1 N2 R
                if let Some(Token::Number(generation_num)) = &self.buf1 {
                    if let Some(Token::Command(cmd)) = &self.buf2 {
                        if cmd == "R" {
                            // This is an indirect reference
                            let num = n as u32;
                            let generation = *generation_num as u32;

                            self.shift()?; // Consume generation number
                            self.shift()?; // Consume 'R'

                            return Ok(PDFObject::Ref { num, generation });
                        }
                    }
                }

                // Not an indirect reference, just a number
                Ok(PDFObject::Number(n))
            }

            // All other simple types can be converted directly
            Token::EOF => Ok(PDFObject::EOF),
            Token::Boolean(b) => Ok(PDFObject::Boolean(b)),
            Token::Null => Ok(PDFObject::Null),
            Token::String(s) => Ok(PDFObject::String(s)),
            Token::HexString(s) => Ok(PDFObject::HexString(s)),
            Token::Name(n) => Ok(PDFObject::Name(n)),
            Token::Command(c) => Ok(PDFObject::Command(c)), // Keep as Command for content streams
        }
    }

    /// Parses an array: [ obj1 obj2 ... ]
    fn parse_array(&mut self) -> PDFResult<PDFObject> {
        let mut array = Vec::new();

        loop {
            // Check if we've reached the end of the array
            if let Some(Token::ArrayEnd) = &self.buf1 {
                self.shift()?; // Consume the ']'
                break;
            }

            // Check for EOF (error: unterminated array)
            if let Some(Token::EOF) = &self.buf1 {
                return Err(PDFError::Generic("Unterminated array (missing ']')".to_string()));
            }

            // Parse the next object in the array with error recovery
            match self.get_object() {
                Ok(obj) => array.push(obj),
                Err(e) => {
                    // Try to recover by inserting null and continuing
                    eprintln!("Warning: Error parsing array element: {:?}, using null", e);
                    array.push(PDFObject::Null);
                    // Try to recover by finding the next token that looks like array end
                    // Skip ahead until we find ']' or some reasonable stopping point
                    let mut recovery_attempts = 0;
                    while recovery_attempts < 10 {
                        if let Some(Token::ArrayEnd) = &self.buf1 {
                            break;
                        }
                        if let Some(Token::EOF) = &self.buf1 {
                            break;
                        }
                        if recovery_attempts > 0 {
                            self.shift()?; // Try to advance
                        }
                        recovery_attempts += 1;
                    }
                }
            }
        }

        Ok(PDFObject::Array(array))
    }

    /// Parses a dictionary: << /Key1 value1 /Key2 value2 ... >>
    fn parse_dictionary(&mut self) -> PDFResult<PDFObject> {
        let mut dict = HashMap::new();

        loop {
            // Check if we've reached the end of the dictionary
            if let Some(Token::DictEnd) = &self.buf1 {
                // DON'T shift yet - we need to check buf2 for "stream" first
                // (matches PDF.js behavior)
                break;
            }

            // Check for EOF (error: unterminated dictionary)
            if let Some(Token::EOF) = &self.buf1 {
                return Err(PDFError::Generic(
                    "Unterminated dictionary (missing '>>')".to_string(),
                ));
            }

            // The key must be a name
            let key = match &self.buf1 {
                Some(Token::Name(name)) => name.clone(),
                Some(Token::Command(cmd)) => {
                    // Commands might be keys in some malformed PDFs
                    cmd.clone()
                }
                Some(other) => {
                    // Malformed dictionary: try to recover by treating other tokens as keys
                    match other {
                        Token::Number(n) => format!("{}", *n),
                        Token::Boolean(b) => format!("{}", b),
                        Token::String(s) => String::from_utf8_lossy(s).to_string(),
                        Token::HexString(s) => format!("<{:?}>", s),
                        _ => {
                            // Skip this token and continue
                            self.shift()?;
                            continue;
                        }
                    }
                }
                None => {
                    return Err(PDFError::Generic(
                        "Unexpected empty buffer in dictionary parsing".to_string(),
                    ))
                }
            };

            self.shift()?; // Consume the key

            // Check if we have a value (could be EOF or >>)
            if let Some(Token::EOF) = &self.buf1 {
                return Err(PDFError::Generic(
                    "Unterminated dictionary (EOF after key)".to_string(),
                ));
            }

            if let Some(Token::DictEnd) = &self.buf1 {
                // Dictionary ended without value for this key - insert null
                dict.insert(key, PDFObject::Null);
                break;
            }

            // Parse the value with error recovery
            let value = match self.get_object() {
                Ok(val) => val,
                Err(e) => {
                    // Try to recover from parsing errors by inserting null
                    // and continuing with the next key-value pair
                    eprintln!("Warning: Error parsing dictionary value for key '{}': {:?}, using null", key, e);
                    PDFObject::Null
                }
            };
            dict.insert(key, value);
        }

        // Check if this dictionary is followed by a stream
        // Format: << /Key value >> stream\n...binary data...endstream
        // IMPORTANT: PDF.js checks buf2 (the lookahead token) for "stream"
        // At this point, buf1='>>',  buf2=next token (possibly "stream")
        // We haven't consumed '>>' yet, so the lexer is positioned correctly.

        if let Some(Token::Command(cmd)) = &self.buf2 {
            if cmd == "stream" {
                // Detected stream! This matches PDF.js's approach.
                // buf1 = '>>' (not yet consumed)
                // buf2 = "stream" token
                // lexer is positioned right after "stream"

                // DON'T call shift() - it would try to fill buf2 from stream data!
                // Instead, just clear both buffers and let parse_stream handle it.
                self.buf1 = None;
                self.buf2 = None;

                // Pass the dictionary to parse_stream which will skip to next line
                return self.parse_stream(dict);
            }
        }

        // Not a stream - just a regular dictionary
        // Consume the '>>' and return the dictionary
        self.shift()?;
        Ok(PDFObject::Dictionary(dict))
    }

    /// Parses a stream object (dictionary followed by stream data).
    ///
    /// Format:
    /// ```text
    /// << /Length 100 >> stream
    /// ...binary data...
    /// endstream
    /// ```
    ///
    /// Based on PDF.js Parser.makeStream()
    ///
    /// CRITICAL ISSUE: When this is called, buf1="stream" and buf2 has already consumed
    /// bytes from the stream data (because get_object() skips whitespace including 0x00).
    /// We need to scan backward or use heuristics to find the actual stream start.
    fn parse_stream(&mut self, dict: HashMap<String, PDFObject>) -> PDFResult<PDFObject> {
        // When this is called, buf1 and buf2 have already been cleared by parse_dictionary.
        // The lexer is positioned right after the "stream" keyword.
        // PDF.js approach: Skip forward to the next line from the current lexer position.
        // This is simpler and more reliable than searching backward.

        // Skip to the next line (consume bytes until we hit CR or LF, then consume the newline)
        // This is equivalent to PDF.js's lexer.skipToNextLine()
        loop {
            match self.lexer.get_stream_byte() {
                Ok(0x0D) => {
                    // CR - check if next is LF
                    if let Ok(0x0A) = self.lexer.get_stream_byte() {
                        // CR+LF consumed
                    }
                    // Either way, we're done
                    break;
                }
                Ok(0x0A) => {
                    // LF - consumed
                    break;
                }
                Ok(_) => {
                    // Some other character, keep going
                    continue;
                }
                Err(_) => {
                    // EOF or error
                    break;
                }
            }
        }

        // Now we're positioned right after the newline following "stream"
        // This is where the stream data starts
        let stream_start_pos = self.lexer.get_position();

        // Get the Length from the dictionary
        let length = dict
            .get("Length")
            .and_then(|obj| match obj {
                PDFObject::Number(n) => Some(*n as usize),
                PDFObject::Ref { num, generation } => {
                    // Length is an indirect reference - try to resolve it if we have a resolver
                    if let Some(ref resolver) = self.ref_resolver {
                        match resolver(*num, *generation) {
                            Ok(resolved) => match resolved {
                                PDFObject::Number(n) => Some(n as usize),
                                _ => {
                                    eprintln!("Warning: Resolved /Length is not a number, scanning for endstream");
                                    None
                                }
                            },
                            Err(e) => {
                                eprintln!("Warning: Failed to resolve /Length reference {} {} R: {:?}, scanning for endstream", num, generation, e);
                                None
                            }
                        }
                    } else {
                        // No resolver available, fall back to scanning
                        eprintln!("Warning: /Length is an indirect reference but no resolver available, scanning for endstream");
                        None
                    }
                }
                _ => None,
            });

        // Read the stream data
        // We're already positioned at stream_start_pos (right after the newline)
        let data = if let Some(len) = length {
            // We know the length, read exactly that many bytes
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                match self.lexer.get_stream_byte() {
                    Ok(b) => bytes.push(b),
                    Err(_) => break, // EOF or error
                }
            }
            bytes
        } else {
            // No length or indirect length - scan for 'endstream'
            // This is a simplified implementation
            let mut bytes = Vec::new();
            let endstream_marker = b"endstream";
            let mut match_pos = 0;

            loop {
                match self.lexer.get_stream_byte() {
                    Ok(b) => {
                        bytes.push(b);

                        // Check if we're matching 'endstream'
                        if b == endstream_marker[match_pos] {
                            match_pos += 1;
                            if match_pos == endstream_marker.len() {
                                // Found endstream - remove it from bytes
                                bytes.truncate(bytes.len() - endstream_marker.len());
                                // Also trim trailing whitespace before endstream
                                while bytes.last() == Some(&b'\n')
                                    || bytes.last() == Some(&b'\r')
                                    || bytes.last() == Some(&b' ')
                                {
                                    bytes.pop();
                                }
                                break;
                            }
                        } else {
                            match_pos = 0;
                        }
                    }
                    Err(_) => {
                        return Err(PDFError::Generic(
                            "EOF while reading stream data".to_string(),
                        ))
                    }
                }
            }
            bytes
        };

        // Skip past 'endstream' if we haven't already
        // We need to refill the token buffer
        self.buf1 = Some(self.lexer.get_object()?);
        self.buf2 = Some(self.lexer.get_object()?);

        Ok(PDFObject::Stream { dict, data })
    }

    /// Checks if there are more objects to parse.
    pub fn has_more(&self) -> bool {
        !matches!(&self.buf1, Some(Token::EOF))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Stream;

    fn parse_string(input: &str) -> PDFResult<PDFObject> {
        let data = input.as_bytes().to_vec();
        let stream = Box::new(Stream::from_bytes(data));
        let lexer = Lexer::new(stream)?;
        let mut parser = Parser::new(lexer)?;
        parser.get_object()
    }

    #[test]
    fn test_parse_number() {
        let obj = parse_string("42").unwrap();
        assert_eq!(obj, PDFObject::Number(42.0));
    }

    #[test]
    fn test_parse_boolean() {
        assert_eq!(parse_string("true").unwrap(), PDFObject::Boolean(true));
        assert_eq!(parse_string("false").unwrap(), PDFObject::Boolean(false));
    }

    #[test]
    fn test_parse_null() {
        assert_eq!(parse_string("null").unwrap(), PDFObject::Null);
    }

    #[test]
    fn test_parse_string() {
        let obj = parse_string("(hello)").unwrap();
        assert_eq!(obj, PDFObject::String(b"hello".to_vec()));
    }

    #[test]
    fn test_parse_hex_string() {
        let obj = parse_string("<48656c6c6f>").unwrap();
        assert_eq!(obj, PDFObject::HexString(b"Hello".to_vec()));
    }

    #[test]
    fn test_parse_name() {
        let obj = parse_string("/Type").unwrap();
        assert_eq!(obj, PDFObject::Name("Type".to_string()));
    }

    #[test]
    fn test_parse_empty_array() {
        let obj = parse_string("[]").unwrap();
        assert_eq!(obj, PDFObject::Array(vec![]));
    }

    #[test]
    fn test_parse_simple_array() {
        let obj = parse_string("[1 2 3]").unwrap();
        assert_eq!(
            obj,
            PDFObject::Array(vec![
                PDFObject::Number(1.0),
                PDFObject::Number(2.0),
                PDFObject::Number(3.0),
            ])
        );
    }

    #[test]
    fn test_parse_mixed_array() {
        let obj = parse_string("[1 /Name (string) true]").unwrap();
        assert_eq!(
            obj,
            PDFObject::Array(vec![
                PDFObject::Number(1.0),
                PDFObject::Name("Name".to_string()),
                PDFObject::String(b"string".to_vec()),
                PDFObject::Boolean(true),
            ])
        );
    }

    #[test]
    fn test_parse_nested_array() {
        let obj = parse_string("[[1 2] [3 4]]").unwrap();
        assert_eq!(
            obj,
            PDFObject::Array(vec![
                PDFObject::Array(vec![PDFObject::Number(1.0), PDFObject::Number(2.0),]),
                PDFObject::Array(vec![PDFObject::Number(3.0), PDFObject::Number(4.0),]),
            ])
        );
    }

    #[test]
    fn test_parse_empty_dictionary() {
        let obj = parse_string("<<>>").unwrap();
        assert_eq!(obj, PDFObject::Dictionary(HashMap::new()));
    }

    #[test]
    fn test_parse_simple_dictionary() {
        let obj = parse_string("<< /Type /Font >>").unwrap();
        let mut expected = HashMap::new();
        expected.insert("Type".to_string(), PDFObject::Name("Font".to_string()));
        assert_eq!(obj, PDFObject::Dictionary(expected));
    }

    #[test]
    fn test_parse_dictionary_with_multiple_keys() {
        let obj = parse_string("<< /Type /Font /Size 12 /Bold true >>").unwrap();
        let dict = match obj {
            PDFObject::Dictionary(d) => d,
            _ => panic!("Expected dictionary"),
        };

        assert_eq!(
            dict.get("Type"),
            Some(&PDFObject::Name("Font".to_string()))
        );
        assert_eq!(dict.get("Size"), Some(&PDFObject::Number(12.0)));
        assert_eq!(dict.get("Bold"), Some(&PDFObject::Boolean(true)));
    }

    #[test]
    fn test_parse_nested_dictionary() {
        let obj = parse_string("<< /Outer << /Inner 42 >> >>").unwrap();
        let outer_dict = match obj {
            PDFObject::Dictionary(d) => d,
            _ => panic!("Expected dictionary"),
        };

        let inner_obj = outer_dict.get("Outer").unwrap();
        let inner_dict = match inner_obj {
            PDFObject::Dictionary(d) => d,
            _ => panic!("Expected nested dictionary"),
        };

        assert_eq!(inner_dict.get("Inner"), Some(&PDFObject::Number(42.0)));
    }

    #[test]
    fn test_parse_dictionary_with_array_value() {
        let obj = parse_string("<< /Array [1 2 3] >>").unwrap();
        let dict = match obj {
            PDFObject::Dictionary(d) => d,
            _ => panic!("Expected dictionary"),
        };

        let array_obj = dict.get("Array").unwrap();
        assert_eq!(
            array_obj,
            &PDFObject::Array(vec![
                PDFObject::Number(1.0),
                PDFObject::Number(2.0),
                PDFObject::Number(3.0),
            ])
        );
    }

    #[test]
    fn test_parse_indirect_reference() {
        let obj = parse_string("5 0 R").unwrap();
        assert_eq!(obj, PDFObject::Ref { num: 5, generation: 0 });
    }

    #[test]
    fn test_parse_indirect_reference_with_generation() {
        let obj = parse_string("10 2 R").unwrap();
        assert_eq!(obj, PDFObject::Ref { num: 10, generation: 2 });
    }

    #[test]
    fn test_parse_array_with_references() {
        let obj = parse_string("[5 0 R 10 2 R]").unwrap();
        assert_eq!(
            obj,
            PDFObject::Array(vec![
                PDFObject::Ref { num: 5, generation: 0 },
                PDFObject::Ref { num: 10, generation: 2 },
            ])
        );
    }

    #[test]
    fn test_parse_dictionary_with_reference() {
        let obj = parse_string("<< /Parent 5 0 R >>").unwrap();
        let dict = match obj {
            PDFObject::Dictionary(d) => d,
            _ => panic!("Expected dictionary"),
        };

        assert_eq!(
            dict.get("Parent"),
            Some(&PDFObject::Ref { num: 5, generation: 0 })
        );
    }

    #[test]
    fn test_parse_complex_structure() {
        let input = "<< /Type /Page /Contents [5 0 R 6 0 R] /Resources << /Font 7 0 R >> >>";
        let obj = parse_string(input).unwrap();

        let dict = match obj {
            PDFObject::Dictionary(d) => d,
            _ => panic!("Expected dictionary"),
        };

        // Check Type
        assert_eq!(
            dict.get("Type"),
            Some(&PDFObject::Name("Page".to_string()))
        );

        // Check Contents array
        let contents = match dict.get("Contents") {
            Some(PDFObject::Array(arr)) => arr,
            _ => panic!("Expected Contents to be an array"),
        };
        assert_eq!(contents.len(), 2);

        // Check Resources dictionary
        let resources = match dict.get("Resources") {
            Some(PDFObject::Dictionary(d)) => d,
            _ => panic!("Expected Resources to be a dictionary"),
        };
        assert_eq!(
            resources.get("Font"),
            Some(&PDFObject::Ref { num: 7, generation: 0 })
        );
    }

    #[test]
    fn test_unterminated_array() {
        let result = parse_string("[1 2 3");
        assert!(result.is_err());
    }

    #[test]
    fn test_unterminated_dictionary() {
        let result = parse_string("<< /Type /Font");
        assert!(result.is_err());
    }
}
