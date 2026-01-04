use thiserror::Error;

pub struct Color {
    // Represents the color in two bytes of RGB 5-6-5 little endian.
    pub bitwise_color: (u8, u8),
}

impl Color {
    pub const WHITE: Self = Color { bitwise_color: (0xFF, 0xFF) };
    pub const BLACK: Self = Color { bitwise_color: (0x00, 0x00) };
    pub const RED: Self = Color::from_rgb888(255, 0, 0);
    pub const GREEN: Self = Color::from_rgb888(0, 255, 0);
    pub const BLUE: Self = Color::from_rgb888(0, 0, 255);
    pub const YELLOW: Self = Color::from_rgb888(255, 255, 0);
    pub const CYAN: Self = Color::from_rgb888(0, 255, 255);
    pub const MAGENTA: Self = Color::from_rgb888(255, 0, 255);

    /// Converts the given RGB 8-8-8 color to RGB 5-6-5 which is
    /// the internal representation.
    pub const fn from_rgb888(r: u8, g: u8, b: u8) -> Self {
        // Convert each channel to the correct bit size:
        // - Red: 8 bits to 5 bits.
        // - Green: 8 bits to 6 bits.
        // - Blue: 8 bits to 5 bits.
        let r5 = (r >> 3) & 0x1F;
        let g6 = (g >> 2) & 0x3F;
        let b5 = (b >> 3) & 0x1F;
        
        // Combine the channels into a single 16-bit number:
        // The layout is: RRRRR-GGGGGG-BBBBB
        let rgb565: u16 = ((r5 as u16) << 11) | ((g6 as u16) << 5) | (b5 as u16);
        
        // Split the 16-bit color into two bytes big-endian (LSB, MSB).
        Self { bitwise_color: ((rgb565 & 0xFF) as u8, (rgb565 >> 8) as u8) }
    }
}

#[derive(Error, Debug)]
pub enum BufErr {
    #[error("BufErr::DeviceErr: {error_description}")]
    DeviceErr { error_description: String },

    #[error("BufErr::IOError: {0}")]
    Io(#[from] std::io::Error),

    #[error("BufErr::OutOfBounds: ({px}, {py}) was out of the bounds ({bx}, {by}).")]
    OutOfBounds { px: usize, py: usize, bx: usize, by: usize }
}

/// Represents a device with a buffer that can be copied to a render device.
pub trait BufferedRD {
    const WIDTH: usize;
    const HEIGHT: usize;

    /// Draws a pixel on the buffer at x, y. (0, 0) is defined
    /// as the upper-left corner.
    fn draw(&mut self, x: usize, y: usize, color: &Color);

    /// Loads the buffer to the render device.
    fn update(&mut self);

    /// Retrieves a pixel on the buffer at x, y. (0, 0) is defined
    /// as the upper-left corner.
    fn get(&self, x: usize, y: usize) -> Result<Color, BufErr>;

    /// Clears the buffer and resets all pixels to white.
    fn clear(&mut self);
}

pub mod shapes {
    use super::{BufferedRD, Color};

    /// Fills the screen with a singular color.
    pub fn fill<S: BufferedRD>(color: &Color, screen: &mut S) {
        for x in 0..S::WIDTH {
            for y in 0..S::HEIGHT {
                screen.draw(x, y, color);
            }
        }
    }
}