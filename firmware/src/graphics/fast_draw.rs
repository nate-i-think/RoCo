use crate::graphics::{display::FrameBufferData, geometry::Rect};

pub fn draw_rect_xrgb8888(fb: &mut [u8], fb_data: &FrameBufferData, rect: Rect, color: u32) {
    let words_per_row = fb_data.words_per_row();

    // Clip rect against framebuffer bounds.
    let x0 = rect.left.max(0) as usize;
    let y0 = rect.top.max(0) as usize;

    let x1 = (x0 + rect.width as usize).min(fb_data.width);
    let y1 = (y0 + rect.height as usize).min(fb_data.height);

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for yy in y0..y1 {
        let row_start = yy * fb_data.pitch_bytes;
        let row_bytes = &mut fb[row_start..row_start + fb_data.pitch_bytes];

        // 4x speed increase over writing singular bytes.
        let row: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(row_bytes.as_mut_ptr() as *mut u32, words_per_row)
        };

        row[x0..x1].fill(color);
    }
}
