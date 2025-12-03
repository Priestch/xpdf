use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};
use std::sync::Arc;

/// A simple in-memory stream implementation.
///
/// This is the basic stream type that holds PDF data in memory.
/// It serves as a foundation for other stream types and is useful
/// for testing and when the entire PDF is already in memory.
///
/// The underlying data is stored in an Arc, allowing sub-streams to
/// share the same data without cloning.
pub struct Stream {
    /// The underlying byte buffer (shared via Arc)
    bytes: Arc<Vec<u8>>,
    /// Current read position
    pos: usize,
    /// Starting offset in the buffer
    start: usize,
    /// Length of accessible data from start
    length: usize,
}

impl Stream {
    /// Creates a new Stream from a byte vector.
    ///
    /// # Arguments
    /// * `bytes` - The byte data for this stream
    /// * `start` - Starting offset in the byte array (default: 0)
    /// * `length` - Length of accessible data (default: bytes.len())
    pub fn new(bytes: Vec<u8>, start: usize, length: usize) -> Self {
        let actual_length = if length == 0 {
            bytes.len().saturating_sub(start)
        } else {
            length
        };

        Stream {
            bytes: Arc::new(bytes),
            pos: start,
            start,
            length: actual_length,
        }
    }

    /// Creates a new Stream from an Arc-wrapped byte vector.
    ///
    /// This is used internally for creating sub-streams that share data.
    fn from_arc(bytes: Arc<Vec<u8>>, start: usize, length: usize) -> Self {
        Stream {
            bytes,
            pos: start,
            start,
            length,
        }
    }

    /// Creates a new Stream from a byte vector with default parameters.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let length = bytes.len();
        Self::new(bytes, 0, length)
    }

    /// Returns a reference to the underlying byte buffer.
    pub fn get_bytes_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl BaseStream for Stream {
    fn length(&self) -> usize {
        self.length
    }

    fn is_empty(&self) -> bool {
        self.length == 0
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn set_pos(&mut self, pos: usize) -> PDFResult<()> {
        if pos > self.start + self.length {
            return Err(PDFError::InvalidPosition {
                pos,
                length: self.length,
            });
        }
        self.pos = pos;
        Ok(())
    }

    fn get_byte(&mut self) -> PDFResult<u8> {
        if self.pos >= self.start + self.length {
            return Err(PDFError::UnexpectedEndOfStream);
        }
        let byte = self.bytes[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let end_pos = self.pos + length;
        let max_pos = self.start + self.length;

        if end_pos > max_pos {
            return Err(PDFError::UnexpectedEndOfStream);
        }

        let bytes = self.bytes[self.pos..end_pos].to_vec();
        self.pos = end_pos;
        Ok(bytes)
    }

    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>> {
        if begin >= end {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        let max_pos = self.start + self.length;
        if end > max_pos {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        Ok(self.bytes[begin..end].to_vec())
    }

    fn reset(&mut self) -> PDFResult<()> {
        self.pos = self.start;
        Ok(())
    }

    fn move_start(&mut self) -> PDFResult<()> {
        if self.pos > self.start {
            let offset = self.pos - self.start;
            self.start = self.pos;
            self.length = self.length.saturating_sub(offset);
        }
        Ok(())
    }

    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>> {
        if start + length > self.start + self.length {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        // Share the Arc instead of cloning the data
        Ok(Box::new(Stream::from_arc(
            Arc::clone(&self.bytes),
            start,
            length,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let stream = Stream::from_bytes(data.clone());

        assert_eq!(stream.length(), 5);
        assert_eq!(stream.pos(), 0);
        assert!(!stream.is_empty());
    }

    #[test]
    fn test_get_byte() {
        let data = vec![10, 20, 30, 40, 50];
        let mut stream = Stream::from_bytes(data);

        assert_eq!(stream.get_byte().unwrap(), 10);
        assert_eq!(stream.get_byte().unwrap(), 20);
        assert_eq!(stream.pos(), 2);
    }

    #[test]
    fn test_get_bytes() {
        let data = vec![1, 2, 3, 4, 5];
        let mut stream = Stream::from_bytes(data);

        let bytes = stream.get_bytes(3).unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
        assert_eq!(stream.pos(), 3);
    }

    #[test]
    fn test_peek_byte() {
        let data = vec![10, 20, 30];
        let mut stream = Stream::from_bytes(data);

        assert_eq!(stream.peek_byte().unwrap(), 10);
        assert_eq!(stream.pos(), 0); // Position should not change
        assert_eq!(stream.get_byte().unwrap(), 10);
        assert_eq!(stream.pos(), 1);
    }

    #[test]
    fn test_get_uint16() {
        let data = vec![0x12, 0x34, 0x56, 0x78];
        let mut stream = Stream::from_bytes(data);

        assert_eq!(stream.get_uint16().unwrap(), 0x1234);
        assert_eq!(stream.get_uint16().unwrap(), 0x5678);
    }

    #[test]
    fn test_get_int32() {
        let data = vec![0x12, 0x34, 0x56, 0x78];
        let mut stream = Stream::from_bytes(data);

        assert_eq!(stream.get_int32().unwrap(), 0x12345678);
    }

    #[test]
    fn test_reset() {
        let data = vec![1, 2, 3, 4, 5];
        let mut stream = Stream::from_bytes(data);

        stream.get_byte().unwrap();
        stream.get_byte().unwrap();
        assert_eq!(stream.pos(), 2);

        stream.reset().unwrap();
        assert_eq!(stream.pos(), 0);
    }

    #[test]
    fn test_skip() {
        let data = vec![1, 2, 3, 4, 5];
        let mut stream = Stream::from_bytes(data);

        stream.skip(2).unwrap();
        assert_eq!(stream.get_byte().unwrap(), 3);
    }

    #[test]
    fn test_end_of_stream() {
        let data = vec![1, 2];
        let mut stream = Stream::from_bytes(data);

        stream.get_byte().unwrap();
        stream.get_byte().unwrap();
        assert!(stream.get_byte().is_err());
    }

    #[test]
    fn test_sub_stream() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let stream = Stream::from_bytes(data);

        let mut sub = stream.make_sub_stream(2, 4).unwrap();
        assert_eq!(sub.length(), 4);
        assert_eq!(sub.get_byte().unwrap(), 3);
        assert_eq!(sub.get_byte().unwrap(), 4);
    }

    #[test]
    fn test_sub_stream_shares_data() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let stream = Stream::from_bytes(data);

        // Create two sub-streams
        let sub1 = stream.make_sub_stream(0, 5).unwrap();
        let sub2 = stream.make_sub_stream(5, 5).unwrap();

        // They should share the same underlying Arc
        assert_eq!(Arc::strong_count(&stream.bytes), 3); // stream + sub1 + sub2
    }
}
