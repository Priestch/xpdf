use super::base_stream::BaseStream;
use super::error::{PDFError, PDFResult};
use std::sync::{Arc, Mutex};

/// A sub-stream that provides a restricted view into a parent stream.
///
/// This is essential for PDF parsing, as PDFs use sub-streams extensively for:
/// - Object streams (compressed PDF objects)
/// - Content streams (page rendering instructions)
/// - Image data streams
/// - Font data streams
/// - Filtered/decoded streams
///
/// The sub-stream shares data with the parent stream (no copying) and maintains
/// its own position within the restricted range.
pub struct SubStream {
    /// Reference to the parent stream (shared ownership with interior mutability)
    parent: Arc<Mutex<Box<dyn BaseStream>>>,
    /// Absolute starting position in the parent stream
    start: usize,
    /// Length of this sub-stream
    length: usize,
    /// Current position relative to start (0 = start of sub-stream)
    pos: usize,
}

impl SubStream {
    /// Creates a new sub-stream from a parent stream.
    ///
    /// # Arguments
    /// * `parent` - The parent stream (will be wrapped in Arc<Mutex<>>)
    /// * `start` - Starting offset in the parent stream
    /// * `length` - Length of the sub-stream
    pub fn new(parent: Box<dyn BaseStream>, start: usize, length: usize) -> PDFResult<Self> {
        // Validate bounds
        if start + length > parent.length() {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        Ok(SubStream {
            parent: Arc::new(Mutex::new(parent)),
            start,
            length,
            pos: 0,
        })
    }

    /// Creates a sub-stream from an existing Arc<Mutex<>> parent.
    ///
    /// This is useful for creating sub-streams of sub-streams without
    /// creating nested mutex locks.
    pub fn from_shared(
        parent: Arc<Mutex<Box<dyn BaseStream>>>,
        start: usize,
        length: usize,
    ) -> PDFResult<Self> {
        // Validate bounds
        let parent_length = parent.lock().unwrap().length();
        if start + length > parent_length {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        Ok(SubStream {
            parent,
            start,
            length,
            pos: 0,
        })
    }

    /// Returns the absolute position in the parent stream.
    fn absolute_pos(&self) -> usize {
        self.start + self.pos
    }
}

impl BaseStream for SubStream {
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
        if pos > self.length {
            return Err(PDFError::InvalidPosition {
                pos,
                length: self.length,
            });
        }
        self.pos = pos;
        Ok(())
    }

    fn is_data_loaded(&self) -> bool {
        let parent = self.parent.lock().unwrap();
        parent.is_data_loaded()
    }

    fn get_byte(&mut self) -> PDFResult<u8> {
        if self.pos >= self.length {
            return Err(PDFError::UnexpectedEndOfStream);
        }

        let abs_pos = self.absolute_pos();
        let mut parent = self.parent.lock().unwrap();

        // Set parent position and read byte
        parent.set_pos(abs_pos)?;
        let byte = parent.get_byte()?;

        self.pos += 1;
        Ok(byte)
    }

    fn get_bytes(&mut self, length: usize) -> PDFResult<Vec<u8>> {
        let actual_length = std::cmp::min(length, self.length - self.pos);

        if actual_length == 0 {
            return Ok(Vec::new());
        }

        let abs_pos = self.absolute_pos();
        let mut parent = self.parent.lock().unwrap();

        // Set parent position and read bytes
        parent.set_pos(abs_pos)?;
        let bytes = parent.get_bytes(actual_length)?;

        self.pos += bytes.len();
        Ok(bytes)
    }

    fn get_byte_range(&self, begin: usize, end: usize) -> PDFResult<Vec<u8>> {
        if begin >= end {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        if end > self.length {
            return Err(PDFError::InvalidByteRange { begin, end });
        }

        // Translate to absolute positions in parent
        let abs_begin = self.start + begin;
        let abs_end = self.start + end;

        let parent = self.parent.lock().unwrap();
        parent.get_byte_range(abs_begin, abs_end)
    }

    fn reset(&mut self) -> PDFResult<()> {
        self.pos = 0;
        Ok(())
    }

    fn move_start(&mut self) -> PDFResult<()> {
        if self.pos > 0 {
            self.start += self.pos;
            self.length -= self.pos;
            self.pos = 0;
        }
        Ok(())
    }

    fn make_sub_stream(&self, start: usize, length: usize) -> PDFResult<Box<dyn BaseStream>> {
        if start + length > self.length {
            return Err(PDFError::InvalidByteRange {
                begin: start,
                end: start + length,
            });
        }

        // Create a sub-stream relative to this sub-stream's start
        let abs_start = self.start + start;
        let sub = SubStream::from_shared(Arc::clone(&self.parent), abs_start, length)?;

        Ok(Box::new(sub))
    }

    fn get_base_streams(&self) -> Option<Vec<Box<dyn BaseStream>>> {
        // Delegate to parent
        let parent = self.parent.lock().unwrap();
        parent.get_base_streams()
    }

    fn get_original_stream(&self) -> Option<&dyn BaseStream> {
        // We can't return a reference through the mutex
        // This limitation is acceptable as this method is rarely used
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Stream;

    #[test]
    fn test_substream_creation() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let sub = SubStream::new(parent, 2, 5).unwrap();

        assert_eq!(sub.length(), 5);
        assert_eq!(sub.pos(), 0);
        assert!(!sub.is_empty());
    }

    #[test]
    fn test_substream_read_byte() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let mut sub = SubStream::new(parent, 2, 5).unwrap();

        // Sub-stream starts at index 2, so first byte should be 2
        assert_eq!(sub.get_byte().unwrap(), 2);
        assert_eq!(sub.get_byte().unwrap(), 3);
        assert_eq!(sub.pos(), 2);
    }

    #[test]
    fn test_substream_read_bytes() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let mut sub = SubStream::new(parent, 2, 5).unwrap();

        let bytes = sub.get_bytes(3).unwrap();
        assert_eq!(bytes, vec![2, 3, 4]);
        assert_eq!(sub.pos(), 3);
    }

    #[test]
    fn test_substream_bounds() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let mut sub = SubStream::new(parent, 2, 5).unwrap();

        // Read all 5 bytes
        let bytes = sub.get_bytes(5).unwrap();
        assert_eq!(bytes, vec![2, 3, 4, 5, 6]);

        // Try to read past end - should return error
        assert!(sub.get_byte().is_err());
    }

    #[test]
    fn test_substream_of_substream() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let sub1 = SubStream::new(parent, 2, 6).unwrap(); // [2,3,4,5,6,7]
        let mut sub2 = sub1.make_sub_stream(1, 3).unwrap(); // [3,4,5]

        let bytes = sub2.get_bytes(3).unwrap();
        assert_eq!(bytes, vec![3, 4, 5]);
    }

    #[test]
    fn test_substream_reset() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let mut sub = SubStream::new(parent, 2, 5).unwrap();

        sub.get_bytes(3).unwrap();
        assert_eq!(sub.pos(), 3);

        sub.reset().unwrap();
        assert_eq!(sub.pos(), 0);

        // After reset, should read from start again
        assert_eq!(sub.get_byte().unwrap(), 2);
    }

    #[test]
    fn test_substream_get_byte_range() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        let sub = SubStream::new(parent, 2, 5).unwrap();

        // Get range [1, 4) relative to sub-stream (absolute [3, 6))
        let bytes = sub.get_byte_range(1, 4).unwrap();
        assert_eq!(bytes, vec![3, 4, 5]);
    }

    #[test]
    fn test_substream_invalid_bounds() {
        let data = vec![0, 1, 2, 3, 4, 5];
        let stream = Stream::from_bytes(data);
        let parent = Box::new(stream) as Box<dyn BaseStream>;

        // Try to create sub-stream beyond parent bounds
        let result = SubStream::new(parent, 2, 10);
        assert!(result.is_err());
    }
}
