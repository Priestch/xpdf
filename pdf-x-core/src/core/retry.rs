/// Retry macros for exception-driven progressive loading.
///
/// These macros implement PDF.js's exception-driven loading pattern where operations
/// throw DataMissing errors, callers load the required data, and retry the operation.
///
/// This is critical for progressive loading from network sources where we want to:
/// 1. Attempt the operation with currently available data
/// 2. If data is missing, request it (HTTP range request, etc.)
/// 3. Retry the operation once data arrives
///
/// For filesystem sources, this pattern is less critical since chunk loading is fast,
/// but it establishes the architectural pattern for future network support.

/// Retries an operation up to MAX_RETRIES times when DataMissing errors occur.
///
/// # Example
/// ```ignore
/// retry_on_data_missing!(stream, {
///     parser.parse_xref()
/// })
/// ```
///
/// This will:
/// 1. Try to parse_xref()
/// 2. If DataMissing { position, length } is thrown:
///    - Call stream.ensure_range(position, length) to load the data
///    - Retry parse_xref()
/// 3. Repeat up to MAX_RETRIES times
/// 4. Return the result or propagate other errors
#[macro_export]
macro_rules! retry_on_data_missing {
    ($stream:expr, $operation:expr) => {{
        const MAX_RETRIES: usize = 10;
        let mut retries = 0;

        loop {
            match $operation {
                Ok(result) => break Ok(result),
                Err($crate::core::error::PDFError::DataMissing { position, length }) => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        break Err($crate::core::error::PDFError::Generic(format!(
                            "Exceeded maximum retries ({}) while loading data at position {} (length {})",
                            MAX_RETRIES, position, length
                        )));
                    }

                    // Load the missing data
                    $stream.ensure_range(position, length)?;

                    // Loop will retry the operation
                }
                Err(e) => break Err(e),
            }
        }
    }};
}

/// Retries an operation with a custom retry limit.
///
/// # Example
/// ```ignore
/// retry_on_data_missing_with_limit!(stream, 5, {
///     parser.parse_object()
/// })
/// ```
#[macro_export]
macro_rules! retry_on_data_missing_with_limit {
    ($stream:expr, $max_retries:expr, $operation:expr) => {{
        let mut retries = 0;

        loop {
            match $operation {
                Ok(result) => break Ok(result),
                Err($crate::core::error::PDFError::DataMissing { position, length }) => {
                    retries += 1;
                    if retries > $max_retries {
                        break Err($crate::core::error::PDFError::Generic(format!(
                            "Exceeded maximum retries ({}) while loading data at position {} (length {})",
                            $max_retries, position, length
                        )));
                    }

                    // Load the missing data
                    $stream.ensure_range(position, length)?;

                    // Loop will retry the operation
                }
                Err(e) => break Err(e),
            }
        }
    }};
}

#[cfg(test)]
mod tests {
    use crate::core::base_stream::BaseStream;
    use crate::core::error::{PDFError, PDFResult};
    use crate::core::stream::Stream;

    #[test]
    fn test_retry_macro_success() -> PDFResult<()> {
        let data = vec![1, 2, 3, 4, 5];
        let mut stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;

        let result: PDFResult<u8> = retry_on_data_missing!(stream, { stream.get_byte() });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
        Ok(())
    }

    #[test]
    fn test_retry_macro_propagates_other_errors() -> PDFResult<()> {
        let data = vec![1, 2, 3];
        let mut stream = Box::new(Stream::from_bytes(data)) as Box<dyn BaseStream>;

        // Consume all bytes first
        stream.get_byte()?;
        stream.get_byte()?;
        stream.get_byte()?;

        // Now reading should fail with UnexpectedEndOfStream
        let result: PDFResult<u8> = retry_on_data_missing!(stream, { stream.get_byte() });

        assert!(result.is_err());
        // Should be UnexpectedEndOfStream, not DataMissing
        match result {
            Err(PDFError::UnexpectedEndOfStream) => {}
            _ => panic!("Expected UnexpectedEndOfStream, got {:?}", result),
        }
        Ok(())
    }
}
