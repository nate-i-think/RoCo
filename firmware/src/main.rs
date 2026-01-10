#![cfg_attr(debug_assertions, allow(unused_variables, dead_code))]

mod ubx;
mod gps;
mod display;
mod graphics;
mod utils;
mod at;
mod cell;
mod text;

pub fn get_number() -> Option<i32> {
    use std::io::{self, Write, BufRead};

    io::stdout().flush().ok()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input).ok()?;
    input.trim().parse().ok()
}

use tokio::time::{sleep, Duration};
use std::{path::Path, time::Instant};

use display::{KMSDisplay, wait_for_pageflip, clear_xrgb8888};

use crate::text::FontAtlas;

/// Main entrypoint. The tokio flavor is kept as `current_thread` as
/// to confine the whole program to one thread for power efficiency
/// and because we're I/O-bound anyway.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Create the 800x480 display.
    let mut display = KMSDisplay::new(800, 480);

    let start = Instant::now();
    let mut x: i32 = 0;
    let mut dir: i32 = 1;

    let atlas = init_text();
    println!("Font load/create time {}ms", start.elapsed().as_millis());

    loop {
        {
            let buf = &mut display.back.map[..];
            let pitch = display.back.pitch;
            let w = 800usize;
            let h = 480usize;

            // Clear to dark gray``
            clear_xrgb8888(buf, pitch, w, h, 0x00202020);

            // Draw a moving rectangle
            let rect_w = 120;
            let rect_h = 60;
            draw_rect_xrgb8888(
                buf,
                pitch,
                w,
                h,
                x as isize,
                200,
                rect_w,
                rect_h,
                0x00FF2020, // red
            );

            render(&atlas, buf, pitch, w, h);
            
        }

        // Queue a page flip
        display.queue_flip();
        // Wait for vblank event asynchronously
        wait_for_pageflip(&display.device).await;
        // Swap front/back roles
        display.swap();

        // Update animation
        x += dir * 8;
        if x <= 0 {
            x = 0;
            dir = 1;
        } else if x + 120 >= 800 as i32 {
            x = 800 as i32 - 120;
            dir = -1;
        }

        // Throttle to ~20 Hz
        sleep(Duration::from_millis(10)).await;
    }
}

fn draw_rect_xrgb8888(
    buf: &mut [u8],
    pitch: usize,
    w: usize,
    h: usize,
    x: isize,
    y: isize,
    rw: usize,
    rh: usize,
    color: u32,
) {
    let words_per_row = pitch / 4;

    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x0 + rw).min(w);
    let y1 = (y0 + rh).min(h);

    for yy in y0..y1 {
        let row_bytes = &mut buf[yy * pitch..yy * pitch + pitch];
        let row: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(row_bytes.as_mut_ptr() as *mut u32, words_per_row)
        };
        row[x0..x1].fill(color);
    }
}

fn init_text() -> FontAtlas {
    let font_name = "RobotoMono-Regular";
    let charset = FontAtlas::ascii_printable();
    FontAtlas::load_or_build_cached(
        font_name,
        text::ROBOTO_MONO_REGULAR,
        32,                       // px
        &charset,
        Path::new("./data/glyphs"),
        512, 512,                 // atlas size
    ).expect("atlas load/build failed")
}

fn render(atlas: &FontAtlas, buf: &mut [u8], pitch: usize, w: usize, h: usize) {
    let top = 20isize;
    let baseline = atlas.baseline_from_top(top);

    atlas.draw_text_blend_xrgb8888(buf, pitch, w, h, 20, baseline, 0x00FFFFFF, "Speed");
    atlas.draw_text_on_solid_bg_xrgb8888(buf, pitch, w, h, 20, baseline + 28, 0x00000000, 0x00FFFFFF, "12.3");
}