use std::error::Error;
use image::GenericImageView;
use image::io::Reader as ImageReader;

use crate::graphics::*;
use crate::display::*;

pub fn blit_image_from_file<D>(display: &mut D, filename: &str) -> Result<(), Box<dyn Error>>
where
    D: BufferedRD,
{
    // Load the image from disk (will decode PNG, JPEG, etc by extension)
    let img = ImageReader::open(filename)?
        .decode()?
        .to_rgb8();

    // Make sure the image matches the display
    let (w, h) = img.dimensions();
    assert_eq!(w as usize, D::WIDTH,  "Image width does not match display");
    assert_eq!(h as usize, D::HEIGHT, "Image height does not match display");

    // Blit every pixel
    for y in 0..D::HEIGHT {
        for x in 0..D::WIDTH {
            let px = img.get_pixel(x as u32, y as u32);
            // px.0 is [R, G, B]
            let color = Color::from_rgb888(px[0], px[1], px[2]);
            display.draw(x, y, &color);
        }
    }

    // Push to hardware
    display.update();

    Ok(())
}
