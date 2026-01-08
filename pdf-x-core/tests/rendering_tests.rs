use pdf_x_core::rendering::{Device, Paint, PathDrawMode};
use pdf_x_core::rendering::skia_device::SkiaDevice;
use pdf_x_core::rendering::graphics_state::{Color, StrokeProps};
use tiny_skia::Pixmap;

#[test]
fn test_draw_rectangle() {
    let mut pixmap = Pixmap::new(100, 100).unwrap();
    let mut device = SkiaDevice::new(pixmap.as_mut());

    device.begin_path();
    device.rect(10.0, 10.0, 80.0, 80.0);
    
    let paint = Paint::Solid(Color::rgb(255, 0, 0));
    device.draw_path(PathDrawMode::Fill(Default::default()), &paint, &StrokeProps::default()).unwrap();

    // Save the pixmap to a file for inspection
    pixmap.save_png("rectangle.png").unwrap();
}
