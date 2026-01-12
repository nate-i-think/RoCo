#![cfg_attr(debug_assertions, allow(unused_variables, dead_code))]

mod graphics;
mod io;
mod debug;

use tokio::time::{sleep, Duration};
use std::{path::Path, time::Instant};

use crate::graphics::color;
use crate::graphics::debug_terminal::DebugTerminal;
use crate::graphics::display::{FrameBufferData, KMSDisplay, clear_xrgb8888, wait_for_pageflip};
use crate::graphics::geometry::Rect;
use crate::graphics::text::FontAtlas;

/// Main entrypoint. The tokio flavor is kept as `current_thread` as
/// to confine the whole program to one thread for power efficiency
/// and because we're I/O-bound anyway.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Create the 800x480 display.
    let mut display = KMSDisplay::new(800, 480);

    let start = Instant::now();
    let atlas = init_text();

    clear_xrgb8888(&mut display.back.map, display.back.pitch, 800, 480, color::WHITE);
    clear_xrgb8888(&mut display.front.map, display.front.pitch, 800, 480, color::WHITE);

    let mut debug_terminal = DebugTerminal::init(
        Rect { left: 0, top: 0, width: 800, height: 480}, 
        &atlas, 
        14, 
        4
    ).unwrap();

    let fb_data = FrameBufferData {
        pitch_bytes: display.back.pitch,
        width: 800,
        height: 480,
    };

    tokio::spawn(test());

    loop {
        let buf = &mut display.back.map[..];
        let w = 800usize;
        let h = 480usize;

        debug_terminal.render(buf, &fb_data);

        // Queue a page flip
        display.queue_flip();
        // Wait for vblank event asynchronously
        wait_for_pageflip(&display.device).await;
        // Swap front/back roles
        display.swap();
        display.copy_front_to_back();

        // Throttle to ~20 Hz
        sleep(Duration::from_millis(10)).await;
    }
}

async fn test() {
    let mut n = 0;
    loop {
        n += 1;
        tracing::info!(target: "TEST", "test {}", n);
        sleep(Duration::from_secs(1)).await;
    }
}

fn init_text() -> FontAtlas {
    let font_name = "RobotoMono-Regular";
    let charset = FontAtlas::ascii_printable();
    FontAtlas::load_or_build_cached(
        font_name,
        graphics::text::ROBOTO_MONO_REGULAR,
        16,                       // px
        &charset,
        Path::new("./data/glyphs"),
        512, 512,                 // atlas size
    ).expect("atlas load/build failed")
}

fn render_test_text(atlas: &FontAtlas, buf: &mut [u8], pitch: usize, w: usize, h: usize) {
    let top = 20isize;
    let baseline = atlas.baseline_from_top(top);

    atlas.draw_text_blend_xrgb8888(buf, pitch, w, h, 20, baseline, 0x00FFFFFF, "Speed");
    atlas.draw_text_on_solid_bg_xrgb8888(buf, pitch, w, h, 20, baseline + 28, 0x00000000, 0x00FFFFFF, "12.3");
}