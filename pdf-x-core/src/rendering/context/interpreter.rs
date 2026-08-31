//! The content-stream interpretation loop shared by pages, forms, and patterns.

use super::*;

impl<'a, D: Device> RenderingContext<'a, D> {
    pub(super) fn process_content_stream_bytes(&mut self, decoded_data: Vec<u8>) -> PDFResult<()> {
        let stream = Box::new(crate::core::Stream::from_bytes(decoded_data))
            as Box<dyn crate::core::BaseStream>;
        let lexer = crate::core::Lexer::new(stream)?;
        let parser = crate::core::Parser::new(lexer)?;
        let mut evaluator = crate::core::ContentStreamEvaluator::new(parser);

        while let Some(op) = evaluator.read_operation()? {
            self.process_operation(&op)?;
        }

        Ok(())
    }
}
