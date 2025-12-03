use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};

/// PDF token types returned by the Lexer.
///
/// This matches the types returned by PDF.js's Lexer.getObj()
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// End of file marker
    EOF,

    /// Boolean value
    Boolean(bool),

    /// Null value
    Null,

    /// Numeric value (integers and reals)
    Number(f64),

    /// String value (from literal strings like (hello))
    String(Vec<u8>),

    /// Hex string value (from hex strings like <48656c6c6f>)
    HexString(Vec<u8>),

    /// Name value (from /Name)
    Name(String),

    /// Command/operator (like 'q', 'Q', 'BT', 'ET', etc.)
    Command(String),

    /// Array start '['
    ArrayStart,

    /// Array end ']'
    ArrayEnd,

    /// Dictionary start '<<'
    DictStart,

    /// Dictionary end '>>'
    DictEnd,
}

/// PDF Lexer for tokenizing PDF syntax.
///
/// This is analogous to PDF.js's Lexer class which tokenizes the PDF byte stream
/// into higher-level tokens like numbers, strings, names, operators, etc.
///
/// The lexer handles:
/// - Whitespace and comment skipping
/// - Number parsing (integers, reals, scientific notation)
/// - String parsing (literal and hexadecimal)
/// - Name parsing
/// - Command/operator parsing
/// - Special characters ([, ], <<, >>, etc.)
pub struct Lexer {
    /// The input stream
    stream: Box<dyn BaseStream>,

    /// Current character being examined
    current_char: i32,

    /// Buffer for building strings
    str_buf: Vec<u8>,
}

impl Lexer {
    /// Creates a new Lexer from a stream.
    pub fn new(mut stream: Box<dyn BaseStream>) -> PDFResult<Self> {
        let current_char = Self::read_char(&mut stream)?;

        Ok(Lexer {
            stream,
            current_char,
            str_buf: Vec::new(),
        })
    }

    /// Reads the next character from the stream.
    ///
    /// Returns -1 on EOF.
    fn read_char(stream: &mut Box<dyn BaseStream>) -> PDFResult<i32> {
        match stream.get_byte() {
            Ok(byte) => Ok(byte as i32),
            Err(PDFError::UnexpectedEndOfStream) => Ok(-1),
            Err(e) => Err(e),
        }
    }

    /// Advances to the next character.
    fn next_char(&mut self) -> PDFResult<i32> {
        self.current_char = Self::read_char(&mut self.stream)?;
        Ok(self.current_char)
    }

    /// Peeks at the next character without consuming it.
    fn peek_char(&mut self) -> PDFResult<i32> {
        match self.stream.peek_byte() {
            Ok(byte) => Ok(byte as i32),
            Err(PDFError::UnexpectedEndOfStream) => Ok(-1),
            Err(e) => Err(e),
        }
    }

    /// Checks if a character is whitespace according to PDF spec.
    ///
    /// PDF whitespace: NUL, TAB, LF, FF, CR, SPACE
    fn is_whitespace(ch: i32) -> bool {
        matches!(ch, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
    }

    /// Checks if a character is a delimiter according to PDF spec.
    ///
    /// PDF delimiters: ( ) < > [ ] { } / %
    fn is_delimiter(ch: i32) -> bool {
        matches!(
            ch,
            0x28 | 0x29 | 0x3C | 0x3E | 0x5B | 0x5D | 0x7B | 0x7D | 0x2F | 0x25
        )
    }

    /// Checks if a character is special (whitespace or delimiter).
    fn is_special(ch: i32) -> bool {
        Self::is_whitespace(ch) || Self::is_delimiter(ch)
    }

    /// Skips whitespace and comments.
    fn skip_whitespace_and_comments(&mut self) -> PDFResult<()> {
        let mut comment = false;

        loop {
            let ch = self.current_char;

            if ch < 0 {
                break;
            }

            if comment {
                // In a comment, skip until newline
                if ch == 0x0A || ch == 0x0D {
                    // LF or CR
                    comment = false;
                }
            } else if ch == 0x25 {
                // '%' starts a comment
                comment = true;
            } else if !Self::is_whitespace(ch) {
                break;
            }

            self.next_char()?;
        }

        Ok(())
    }

    /// Gets the next token/object from the stream.
    ///
    /// This is the main method analogous to PDF.js's Lexer.getObj()
    pub fn get_object(&mut self) -> PDFResult<Token> {
        // Skip whitespace and comments
        self.skip_whitespace_and_comments()?;

        let ch = self.current_char;

        // Check for EOF
        if ch < 0 {
            return Ok(Token::EOF);
        }

        // Match based on first character
        match ch {
            // Numbers: 0-9, +, -, .
            0x30..=0x39 | 0x2B | 0x2D | 0x2E => self.get_number(),

            // Literal string: (
            0x28 => self.get_string(),

            // Name: /
            0x2F => self.get_name(),

            // Array start: [
            0x5B => {
                self.next_char()?;
                Ok(Token::ArrayStart)
            }

            // Array end: ]
            0x5D => {
                self.next_char()?;
                Ok(Token::ArrayEnd)
            }

            // Hex string or dict start: <
            0x3C => {
                let next_ch = self.next_char()?;
                if next_ch == 0x3C {
                    // << dictionary start
                    self.next_char()?;
                    Ok(Token::DictStart)
                } else {
                    // < hex string
                    self.get_hex_string()
                }
            }

            // Dict end: >
            0x3E => {
                let next_ch = self.next_char()?;
                if next_ch == 0x3E {
                    // >> dictionary end
                    self.next_char()?;
                    Ok(Token::DictEnd)
                } else {
                    Err(PDFError::Generic(format!(
                        "Unexpected character: >{}",
                        next_ch
                    )))
                }
            }

            // Closing paren is an error if encountered here
            0x29 => {
                self.next_char()?;
                Err(PDFError::Generic(format!("Illegal character: {}", ch)))
            }

            // Everything else is a command/keyword
            _ => self.get_command(),
        }
    }

    /// Parses a number token.
    ///
    /// Handles integers, reals, and scientific notation.
    /// Based on PDF.js Lexer.getNumber()
    fn get_number(&mut self) -> PDFResult<Token> {
        let mut ch = self.current_char;
        let mut e_notation = false;
        let mut divide_by = 0.0; // Non-zero if it's a floating point value
        let mut sign = 1.0;

        // Handle optional sign
        if ch == 0x2D {
            // '-'
            sign = -1.0;
            ch = self.next_char()?;

            // Ignore double negative (consistent with Adobe Reader)
            if ch == 0x2D {
                ch = self.next_char()?;
            }
        } else if ch == 0x2B {
            // '+'
            ch = self.next_char()?;
        }

        // Ignore line-breaks after sign (consistent with Adobe Reader)
        if ch == 0x0A || ch == 0x0D {
            // LF or CR
            loop {
                ch = self.next_char()?;
                if ch != 0x0A && ch != 0x0D {
                    break;
                }
            }
        }

        // Handle optional leading decimal point
        if ch == 0x2E {
            // '.'
            divide_by = 10.0;
            ch = self.next_char()?;
        }

        // Must have at least one digit
        if ch < 0x30 || ch > 0x39 {
            // Not a digit
            // Return 0 for invalid numbers followed by whitespace/delimiters/EOF
            // (consistent with Adobe Reader)
            if Self::is_whitespace(ch) || ch == 0x28 || ch == 0x3C || ch == -1 {
                return Ok(Token::Number(0.0));
            }
            return Err(PDFError::Generic(format!(
                "Invalid number: {} (charCode {})",
                if ch >= 0 {
                    (ch as u8 as char).to_string()
                } else {
                    "EOF".to_string()
                },
                ch
            )));
        }

        let mut base_value = (ch - 0x30) as f64; // '0'
        let mut power_value = 0;
        let mut power_value_sign = 1;

        // Parse remaining digits
        loop {
            ch = self.next_char()?;
            if ch < 0 {
                break;
            }

            if ch >= 0x30 && ch <= 0x39 {
                // Digit
                let current_digit = (ch - 0x30) as f64;
                if e_notation {
                    // We are after an 'e' or 'E'
                    power_value = power_value * 10 + (ch - 0x30);
                } else {
                    if divide_by != 0.0 {
                        // We are after a decimal point
                        divide_by *= 10.0;
                    }
                    base_value = base_value * 10.0 + current_digit;
                }
            } else if ch == 0x2E {
                // '.'
                if divide_by == 0.0 {
                    divide_by = 1.0;
                } else {
                    // A number can have only one dot
                    break;
                }
            } else if ch == 0x2D {
                // '-' in the middle of number
                // Ignore minus signs in the middle to match Adobe's behavior
                // (just continue parsing)
            } else if ch == 0x45 || ch == 0x65 {
                // 'E' or 'e'
                // Could be scientific notation or beginning of new operator
                let peek_ch = self.peek_char()?;
                if peek_ch == 0x2B || peek_ch == 0x2D {
                    // '+' or '-'
                    power_value_sign = if peek_ch == 0x2D { -1 } else { 1 };
                    self.next_char()?; // Consume the sign
                } else if peek_ch < 0x30 || peek_ch > 0x39 {
                    // Not a digit, so 'E' is beginning of new operator
                    break;
                }
                e_notation = true;
            } else {
                // The last character doesn't belong to this number
                break;
            }
        }

        // Calculate final value
        let mut result = base_value;
        if divide_by != 0.0 {
            result /= divide_by;
        }
        if e_notation {
            result *= 10_f64.powi(power_value_sign * power_value);
        }

        Ok(Token::Number(sign * result))
    }

    /// Parses a literal string token.
    ///
    /// Handles nested parentheses and escape sequences.
    /// Based on PDF.js Lexer.getString()
    fn get_string(&mut self) -> PDFResult<Token> {
        let mut num_paren = 1; // Track nested parentheses
        self.str_buf.clear();

        let mut ch = self.next_char()?; // Consume opening '('

        loop {
            let mut char_buffered = false;

            match ch {
                -1 => {
                    // EOF - unterminated string
                    break;
                }

                0x28 => {
                    // '(' - nested opening paren
                    num_paren += 1;
                    self.str_buf.push(b'(');
                }

                0x29 => {
                    // ')' - closing paren
                    num_paren -= 1;
                    if num_paren == 0 {
                        self.next_char()?; // Consume closing ')'
                        break;
                    }
                    self.str_buf.push(b')');
                }

                0x5C => {
                    // '\' - escape sequence
                    ch = self.next_char()?;
                    match ch {
                        -1 => {
                            // EOF after backslash
                            break;
                        }
                        0x6E => self.str_buf.push(b'\n'), // \n
                        0x72 => self.str_buf.push(b'\r'), // \r
                        0x74 => self.str_buf.push(b'\t'), // \t
                        0x62 => self.str_buf.push(0x08), // \b (backspace)
                        0x66 => self.str_buf.push(0x0C), // \f (form feed)
                        0x5C | 0x28 | 0x29 => {
                            // \\, \(, \)
                            self.str_buf.push(ch as u8);
                        }
                        0x30..=0x37 => {
                            // Octal escape \ddd (1-3 digits)
                            let mut x = (ch & 0x0F) as u8;
                            ch = self.next_char()?;
                            char_buffered = true;

                            if ch >= 0x30 && ch <= 0x37 {
                                x = (x << 3) + (ch & 0x0F) as u8;
                                ch = self.next_char()?;

                                if ch >= 0x30 && ch <= 0x37 {
                                    char_buffered = false;
                                    x = (x << 3) + (ch & 0x0F) as u8;
                                }
                            }
                            self.str_buf.push(x);
                        }
                        0x0D => {
                            // CR - line break, skip it and following LF if present
                            if self.peek_char()? == 0x0A {
                                self.next_char()?;
                            }
                        }
                        0x0A => {
                            // LF - line break, skip it
                        }
                        _ => {
                            // Any other character after backslash - just include it
                            self.str_buf.push(ch as u8);
                        }
                    }
                }

                _ => {
                    // Regular character
                    self.str_buf.push(ch as u8);
                }
            }

            if !char_buffered {
                ch = self.next_char()?;
            }
        }

        Ok(Token::String(self.str_buf.clone()))
    }

    /// Converts a hex character to its numeric value.
    ///
    /// Returns -1 if not a valid hex digit.
    fn to_hex_digit(ch: i32) -> i32 {
        if ch >= 0x30 && ch <= 0x39 {
            // '0'-'9'
            ch & 0x0F
        } else if (ch >= 0x41 && ch <= 0x46) || (ch >= 0x61 && ch <= 0x66) {
            // 'A'-'F' or 'a'-'f'
            (ch & 0x0F) + 9
        } else {
            -1
        }
    }

    /// Parses a hex string token.
    ///
    /// Hex strings are enclosed in angle brackets: <48656c6c6f>
    /// Based on PDF.js Lexer.getHexString()
    fn get_hex_string(&mut self) -> PDFResult<Token> {
        self.str_buf.clear();
        let mut ch = self.current_char;
        let mut first_digit = -1;

        loop {
            if ch < 0 {
                // EOF - unterminated hex string
                break;
            } else if ch == 0x3E {
                // '>' - end of hex string
                self.next_char()?;
                break;
            } else if Self::is_whitespace(ch) {
                // Skip whitespace
                ch = self.next_char()?;
                continue;
            } else {
                let digit = Self::to_hex_digit(ch);
                if digit == -1 {
                    // Invalid hex digit - skip it
                } else if first_digit == -1 {
                    first_digit = digit;
                } else {
                    // Two hex digits make one byte
                    self.str_buf.push(((first_digit << 4) | digit) as u8);
                    first_digit = -1;
                }
                ch = self.next_char()?;
            }
        }

        // If there's an odd number of hex digits, assume final digit is 0
        if first_digit != -1 {
            self.str_buf.push((first_digit << 4) as u8);
        }

        Ok(Token::HexString(self.str_buf.clone()))
    }

    /// Parses a name token.
    ///
    /// Names start with '/' and continue until whitespace or delimiter.
    /// Handles '#' escape sequences like #20 for space.
    /// Based on PDF.js Lexer.getName()
    fn get_name(&mut self) -> PDFResult<Token> {
        self.str_buf.clear();

        // Skip the initial '/'
        let mut ch = self.next_char()?;

        while ch >= 0 && !Self::is_special(ch) {
            if ch == 0x23 {
                // '#' - hex escape sequence
                ch = self.next_char()?;

                if Self::is_special(ch) {
                    // # followed by special char - just include the #
                    self.str_buf.push(b'#');
                    break;
                }

                let x = Self::to_hex_digit(ch);
                if x != -1 {
                    // First hex digit is valid
                    let previous_ch = ch;
                    ch = self.next_char()?;
                    let x2 = Self::to_hex_digit(ch);

                    if x2 == -1 {
                        // Second hex digit is invalid
                        self.str_buf.push(b'#');
                        self.str_buf.push(previous_ch as u8);

                        if Self::is_special(ch) {
                            break;
                        }
                        self.str_buf.push(ch as u8);
                        ch = self.next_char()?;
                        continue;
                    }

                    // Both hex digits valid - decode the character
                    self.str_buf.push(((x << 4) | x2) as u8);
                } else {
                    // First digit not valid hex - just include # and the char
                    self.str_buf.push(b'#');
                    self.str_buf.push(ch as u8);
                }
            } else {
                // Regular character
                self.str_buf.push(ch as u8);
            }

            ch = self.next_char()?;
        }

        // Convert bytes to String
        let name = String::from_utf8_lossy(&self.str_buf).to_string();

        Ok(Token::Name(name))
    }

    /// Parses a command/keyword token.
    ///
    /// Reads non-special characters to form keywords or commands.
    /// Handles special keywords: true, false, null
    /// Based on PDF.js Lexer.getObj() command handling
    fn get_command(&mut self) -> PDFResult<Token> {
        let mut str_buf = String::new();
        let mut ch = self.current_char;

        // Read characters until we hit a special character
        while ch >= 0 && !Self::is_special(ch) {
            if str_buf.len() >= 128 {
                return Err(PDFError::Generic(format!(
                    "Command token too long: {}",
                    str_buf.len()
                )));
            }

            str_buf.push(ch as u8 as char);
            ch = self.next_char()?;
        }

        // Check for boolean keywords
        if str_buf == "true" {
            return Ok(Token::Boolean(true));
        }
        if str_buf == "false" {
            return Ok(Token::Boolean(false));
        }
        if str_buf == "null" {
            return Ok(Token::Null);
        }

        // Otherwise it's a command/operator
        Ok(Token::Command(str_buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Stream;

    #[test]
    fn test_lexer_creation() {
        let data = vec![0x25, 0x50, 0x44, 0x46]; // %PDF
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let lexer = Lexer::new(stream);
        assert!(lexer.is_ok());
    }

    #[test]
    fn test_is_whitespace() {
        assert!(Lexer::is_whitespace(0x00)); // NUL
        assert!(Lexer::is_whitespace(0x09)); // TAB
        assert!(Lexer::is_whitespace(0x0A)); // LF
        assert!(Lexer::is_whitespace(0x0C)); // FF
        assert!(Lexer::is_whitespace(0x0D)); // CR
        assert!(Lexer::is_whitespace(0x20)); // SPACE
        assert!(!Lexer::is_whitespace(0x41)); // 'A'
    }

    #[test]
    fn test_is_delimiter() {
        assert!(Lexer::is_delimiter(0x28)); // (
        assert!(Lexer::is_delimiter(0x29)); // )
        assert!(Lexer::is_delimiter(0x3C)); // <
        assert!(Lexer::is_delimiter(0x3E)); // >
        assert!(Lexer::is_delimiter(0x5B)); // [
        assert!(Lexer::is_delimiter(0x5D)); // ]
        assert!(Lexer::is_delimiter(0x2F)); // /
        assert!(Lexer::is_delimiter(0x25)); // %
        assert!(!Lexer::is_delimiter(0x41)); // 'A'
    }

    #[test]
    fn test_eof() {
        let data = vec![];
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();
        let token = lexer.get_object().unwrap();
        assert_eq!(token, Token::EOF);
    }

    #[test]
    fn test_array_tokens() {
        let data = vec![0x5B, 0x20, 0x5D]; // [ ]
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::ArrayStart);
        assert_eq!(lexer.get_object().unwrap(), Token::ArrayEnd);
        assert_eq!(lexer.get_object().unwrap(), Token::EOF);
    }

    #[test]
    fn test_dict_tokens() {
        let data = vec![0x3C, 0x3C, 0x20, 0x3E, 0x3E]; // << >>
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::DictStart);
        assert_eq!(lexer.get_object().unwrap(), Token::DictEnd);
        assert_eq!(lexer.get_object().unwrap(), Token::EOF);
    }

    #[test]
    fn test_skip_comments() {
        let data = vec![
            0x25, 0x20, 0x63, 0x6F, 0x6D, 0x6D, 0x65, 0x6E, 0x74, 0x0A, // % comment\n
            0x5B, // [
        ];
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::ArrayStart);
    }

    #[test]
    fn test_integer() {
        let data = b"123".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(123.0));
    }

    #[test]
    fn test_negative_integer() {
        let data = b"-456".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(-456.0));
    }

    #[test]
    fn test_positive_sign() {
        let data = b"+789".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(789.0));
    }

    #[test]
    fn test_float() {
        let data = b"3.14".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(3.14));
    }

    #[test]
    fn test_float_negative() {
        let data = b"-2.718".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(-2.718));
    }

    #[test]
    fn test_leading_decimal() {
        let data = b".5".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(0.5));
    }

    #[test]
    fn test_scientific_notation() {
        let data = b"1.5e2".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(150.0));
    }

    #[test]
    fn test_scientific_notation_negative_exp() {
        let data = b"3e-2".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(0.03));
    }

    #[test]
    fn test_scientific_notation_uppercase() {
        let data = b"2E3".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(2000.0));
    }

    #[test]
    fn test_double_negative() {
        let data = b"--5".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        // Double negative: first - is sign, second - is ignored
        // Result is still negative (consistent with Adobe Reader/PDF.js)
        assert_eq!(lexer.get_object().unwrap(), Token::Number(-5.0));
    }

    #[test]
    fn test_invalid_number_returns_zero() {
        let data = b"- ".to_vec(); // Just a minus followed by whitespace
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        // Should return 0 (consistent with Adobe Reader)
        assert_eq!(lexer.get_object().unwrap(), Token::Number(0.0));
    }

    #[test]
    fn test_multiple_numbers() {
        let data = b"1 2.5 -3".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Number(1.0));
        assert_eq!(lexer.get_object().unwrap(), Token::Number(2.5));
        assert_eq!(lexer.get_object().unwrap(), Token::Number(-3.0));
    }

    #[test]
    fn test_simple_string() {
        let data = b"(hello)".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::String(b"hello".to_vec()));
    }

    #[test]
    fn test_string_with_spaces() {
        let data = b"(hello world)".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::String(b"hello world".to_vec())
        );
    }

    #[test]
    fn test_nested_parens() {
        let data = b"(hello (nested) world)".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::String(b"hello (nested) world".to_vec())
        );
    }

    #[test]
    fn test_escape_sequences() {
        let data = b"(line1\\nline2\\ttab\\\\backslash)".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::String(b"line1\nline2\ttab\\backslash".to_vec())
        );
    }

    #[test]
    fn test_escaped_parens() {
        let data = b"(\\(\\))".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::String(b"()".to_vec()));
    }

    #[test]
    fn test_octal_escape() {
        let data = b"(\\101\\102\\103)".to_vec(); // ABC in octal
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::String(b"ABC".to_vec()));
    }

    #[test]
    fn test_hex_string() {
        let data = b"<48656c6c6f>".to_vec(); // "Hello" in hex
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::HexString(b"Hello".to_vec())
        );
    }

    #[test]
    fn test_hex_string_uppercase() {
        let data = b"<48454C4C4F>".to_vec(); // "HELLO" in uppercase hex
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::HexString(b"HELLO".to_vec())
        );
    }

    #[test]
    fn test_hex_string_odd_digits() {
        let data = b"<41>".to_vec(); // 'A' - odd number of digits
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::HexString(b"A".to_vec())
        );
    }

    #[test]
    fn test_hex_string_with_whitespace() {
        let data = b"<48 65 6c 6c 6f>".to_vec(); // "Hello" with spaces
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::HexString(b"Hello".to_vec())
        );
    }

    #[test]
    fn test_simple_name() {
        let data = b"/Type".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Type".to_string())
        );
    }

    #[test]
    fn test_name_with_hash_escape() {
        let data = b"/My#20Name".to_vec(); // /My Name (space encoded as #20)
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("My Name".to_string())
        );
    }

    #[test]
    fn test_name_with_slash_escape() {
        let data = b"/A#2FB".to_vec(); // /A/B (slash encoded as #2F)
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("A/B".to_string())
        );
    }

    #[test]
    fn test_multiple_names() {
        let data = b"/Type /Font /Name".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Type".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Font".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Name".to_string())
        );
    }

    #[test]
    fn test_boolean_true() {
        let data = b"true".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Boolean(true));
    }

    #[test]
    fn test_boolean_false() {
        let data = b"false".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Boolean(false));
    }

    #[test]
    fn test_null() {
        let data = b"null".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::Null);
    }

    #[test]
    fn test_commands() {
        let data = b"q Q BT ET".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Command("q".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Command("Q".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Command("BT".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Command("ET".to_string())
        );
    }

    #[test]
    fn test_mixed_tokens() {
        let data = b"<< /Type /Font /Size 12 >>".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::DictStart);
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Type".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Font".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Size".to_string())
        );
        assert_eq!(lexer.get_object().unwrap(), Token::Number(12.0));
        assert_eq!(lexer.get_object().unwrap(), Token::DictEnd);
    }

    #[test]
    fn test_array_with_mixed_types() {
        let data = b"[1 2.5 /Name (string) true]".to_vec();
        let stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;
        let mut lexer = Lexer::new(stream).unwrap();

        assert_eq!(lexer.get_object().unwrap(), Token::ArrayStart);
        assert_eq!(lexer.get_object().unwrap(), Token::Number(1.0));
        assert_eq!(lexer.get_object().unwrap(), Token::Number(2.5));
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::Name("Name".to_string())
        );
        assert_eq!(
            lexer.get_object().unwrap(),
            Token::String(b"string".to_vec())
        );
        assert_eq!(lexer.get_object().unwrap(), Token::Boolean(true));
        assert_eq!(lexer.get_object().unwrap(), Token::ArrayEnd);
    }
}
