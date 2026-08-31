//! Graphics state, matrix, blend, soft mask, opacity (`Device` methods).

use super::*;

impl<'a> SkiaDevice<'a> {
    pub(super) fn save_state(&mut self) {
        let current_state = self.current_state().clone();
        self.state_stack.push(current_state);
    }

    pub(super) fn restore_state(&mut self) {
        if self.state_stack.len() > 1 {
            self.state_stack.pop();
        }
    }

    pub(super) fn concat_matrix(&mut self, matrix: &[f64; 6]) {
        let transform = Transform::from_row(
            matrix[0] as f32,
            matrix[1] as f32,
            matrix[2] as f32,
            matrix[3] as f32,
            matrix[4] as f32,
            matrix[5] as f32,
        );
        // PDF spec: CTM' = M × CTM (multiply on the LEFT)
        let new_transform = self.current_state().transform.pre_concat(transform);
        self.current_state_mut().transform = new_transform;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: concat_matrix: [{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}] -> {:?}",
            matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5], new_transform
        );
    }

    pub(super) fn set_matrix(&mut self, matrix: &[f64; 6]) {
        let new_transform = Transform::from_row(
            matrix[0] as f32,
            matrix[1] as f32,
            matrix[2] as f32,
            matrix[3] as f32,
            matrix[4] as f32,
            matrix[5] as f32,
        );
        self.current_state_mut().transform = new_transform;
        #[cfg(feature = "debug-logging")]
        eprintln!(
            "DEBUG: set_matrix: [{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}] -> {:?}",
            matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5], new_transform
        );
    }

    pub(super) fn set_blend_mode(&mut self, mode: BlendMode) {
        self.current_state_mut().blend_mode = mode;
    }

    pub(super) fn page_bounds(&self) -> (f64, f64) {
        (self.pixmap.width() as f64, self.pixmap.height() as f64)
    }

    pub(super) fn set_soft_mask(&mut self, mask: Option<(&[u8], u32, u32)>) -> PDFResult<()> {
        match mask {
            None => self.current_state_mut().soft_mask = None,
            Some((data, w, h)) => {
                if let Some(size) = tiny_skia::IntSize::from_wh(w, h) {
                    if let Some(m) = Mask::from_vec(data.to_vec(), size) {
                        self.current_state_mut().soft_mask = Some(m);
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn set_opacity(&mut self, opacity: f64) {
        self.current_state_mut().opacity = opacity as f32;
    }
}
