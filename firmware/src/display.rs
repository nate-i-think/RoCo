
use drm::buffer::Buffer;
use drm::control::dumbbuffer::DumbMapping;
use drm::control::{Device as ControlDevice, connector, Event, ModeTypeFlags, PageFlipFlags};
use drm::Device as BasicDevice;

use drm_fourcc::DrmFourcc;

use std::fs::OpenOptions;
use std::os::fd::{AsFd, BorrowedFd};
use std::time::Instant;
use std::os::unix::io::AsRawFd;

use tokio::io::unix::AsyncFd;


#[derive(Debug)]
struct Card(std::fs::File);
impl AsFd for Card { fn as_fd(&self) -> BorrowedFd<'_> { self.0.as_fd() } }
impl AsRawFd for Card { fn as_raw_fd(&self) -> i32 { self.0.as_raw_fd() } }
impl BasicDevice for Card {}
impl ControlDevice for Card {}

/// In DRM/KMS, pixels flow through three hardware blocks:
/// Framebuffer → CRTC → Encoder → Connector → Physical Display.
///  - The CRTC Reads pixels from memory and generates video
/// timing (scanout engine). It literally referrs to Cathode Ray
/// Tube Control, because it used to be for CRTs! I mean, I guess
/// you still could...
/// - The Encoder Converts that timing into a specific signal type
/// (HDMI, DSI, etc.)
/// - The connector is the physical port/panel (`HDMI-A-1`, `DSI-1`,
/// etc.) which you can see at `/sys/class/drm`.
async fn open_card() -> Some(KMSDisplay) {
    // Open DRM device. 
    //
    // I saw v3d (assumed) loaded on card0, 
    // whilst vc4 was loaded on card1, so I could get KMS
    // resources from it.
    //
    // If you recieve some "ERROR OPERATION NOT SUPPORTED"
    // in the future, check `ls -l /dev/dri` in terminal and
    // then swap around which card we look at until you find 
    // the KMS one. Or you can check `ls /sys/class/drm` and
    // see directly to which card hdmi has attached itself.
    let card1 = Card(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/dri/card1")
            .unwrap(),
    );

    // Open the Kernel Mode Setting resources.
    let kms_resources = card1.resource_handles().unwrap();

    // Now we locate a connected connector. Since HDMI is
    // not assigned some nice unique string identifier, but
    // rather a runtime ID, we just have to find it every
    // time. It suffices to locate the only connected
    // connector since the board is hardwired as such.
    let (connector_handle, connector_info) = 
        kms_resources.connectors().iter().find_map(|&h| {
            let info = card1.get_connector(h, false).ok()?;
            if info.state() == connector::State::Connected && !info.modes().is_empty() {
                Some((h, info))
            } else {
                None
            }
        })
        .unwrap();

    // Pick preferred mode if present, failing into the
    // first mode. This will of course just harvest our
    // custom EDID info.
    let mode = connector_info
        .modes()
        .iter()
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .cloned()
        .unwrap_or_else(|| connector_info.modes()[0]);

    // Selects the first encoder capable of driving this
    // (HDMI) connector. Then, gets info about that 
    // encoder. Finally, selects the first CRTC that is
    // compatable with that encoder. There's probably
    // only one on this board...
    let enc_handle = *connector_info.encoders().get(0).unwrap();
    let enc_info = card1.get_encoder(enc_handle).unwrap();
    let crtc_handle = *kms_resources
        .filter_crtcs(enc_info.possible_crtcs())
        .get(0)
        .unwrap();

    // Target size for panel.
    const PANEL_WIDTH: u32 = 800;
    const PANEL_HEIGHT: u32 = 480;

    // Create two dumb buffers (double-buffer).
    let mut first_db = card1
        .create_dumb_buffer((PANEL_WIDTH, PANEL_HEIGHT), DrmFourcc::Xrgb8888, 32)
        .unwrap();
    let mut second_db = card1
        .create_dumb_buffer((PANEL_WIDTH, PANEL_HEIGHT), DrmFourcc::Xrgb8888, 32)
        .unwrap();

    // Cache pitch before mapping (mapping holds a 
    // &mut borrow). Pitch is the number of bytes between
    // the start of one row of pixels and the start of the
    // next row in memory. You'd expect it to be 800px 
    // times 4 bytes per pixel = 3200 bytes, but there's 
    // some padding involved for alignment.
    let mut first_db_pitch = first_db.pitch() as usize;
    let mut second_db_pitch = second_db.pitch() as usize;

    // Add framebuffers (depth=24, bpp=32 for XRGB8888 legacy helper).
    // Take care to note max_framebuffers=2 in /boot/firmware/config.txt.
    let first_fb_handle = card1.add_framebuffer(&first_db, 24, 32).unwrap();
    let second_fb_handle = card1.add_framebuffer(&second_db, 24, 32).unwrap();

    // Map buffers to accessable memory.
    let mut map_front = card1.map_dumb_buffer(&mut first_db).unwrap();
    let mut map_back = card1.map_dumb_buffer(&mut second_db).unwrap();

    // Clear the front buffer so it's not garbage memory or something.
    clear_xrgb8888(
        map_front.as_mut(),
        first_db_pitch,
        PANEL_WIDTH as usize,
        PANEL_HEIGHT as usize,
        0x00,
    );
    
    // Configures a display pipeline, setting a scanout engine (CRTC),
    // read pixels from a front framebuffer, drive this connector, 
    // use this display timing mode. The odd (0, 0) sets the x/y 
    // offset into the framebuffer, which tells the DMS where to begin
    // scanning. This would be changed to pan a display around a larger
    // framebuffer.
    card1.set_crtc(
        crtc_handle,
        Some(first_fb_handle),
        (0, 0),
        &[connector_handle],
        Some(mode),
    ).unwrap();

    // Converts `card1` to a pollable/awaitable file descriptor to allow
    // tokio async.
    let card1 = AsyncFd::new(card1).unwrap();



    // Demo loop at ~10 Hz; change to 50ms for 20Hz, 1000ms for 1Hz, etc.
    //let target_dt = Duration::from_millis(100);

    let mut front_fb_handle = first_fb_handle;
    let mut back_fb_handle = second_fb_handle;

    loop {
        draw_demo_ui(
            map_back.as_mut(),
            second_db_pitch,
            PANEL_WIDTH as usize,
            PANEL_HEIGHT as usize,
            1.0,
        );
        
        card1.get_ref()
            .page_flip(crtc_handle, back_fb_handle, PageFlipFlags::EVENT, None)
            .unwrap();

        wait_for_pageflip(&card1).await;

        // Swap roles
        std::mem::swap(&mut front_fb_handle, &mut back_fb_handle);
        std::mem::swap(&mut map_front, &mut map_back);
        std::mem::swap(&mut first_db_pitch, &mut second_db_pitch);

        break;
    }
}

struct Frame {
    fb: drm::control::framebuffer::Handle,
    pitch: usize,
    map: MmapMut,
}

struct KmsDisplay {
    dev: tokio::io::unix::AsyncFd<Card>,
    crtc: drm::control::crtc::Handle,
    conn: drm::control::connector::Handle,
    mode: drm::control::Mode,

    front: Frame,
    back: Frame,
}

async fn wait_for_pageflip(dev: &AsyncFd<Card>) {
    loop {
        let mut readable = dev.readable().await.unwrap();

        let result = readable.try_io(|_| {
            // Drain events
            let events = dev.get_ref().receive_events()?;
            for ev in events {
                if matches!(ev, Event::PageFlip(_)) {
                    return Ok(true);
                }
            }
            Ok(false)
        });

        match result {
            Ok(Ok(true)) => return,                 // got our flip
            Ok(Ok(false)) => continue,              // readable but not the flip we wanted
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Ok(Err(e)) => panic!("receive_events failed: {e}"),
            Err(_would_block) => continue,          // readiness consumed; await again
        }
    }
}

/// Fill whole buffer with a solid color (0x00RRGGBB for XRGB8888).
fn clear_xrgb8888(buf: &mut [u8], pitch: usize, w: usize, h: usize, color: u32) {
    // pitch is in bytes; each pixel is 4 bytes
    let words_per_row = pitch / 4;

    for y in 0..h {
        let row_bytes = &mut buf[y * pitch..y * pitch + pitch];
        let row: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(row_bytes.as_mut_ptr() as *mut u32, words_per_row)
        };

        // fill only the visible width (ignore padding)
        row[..w].fill(color);
    }
}

/// Tiny “UI-ish” animation: a moving rectangle + a changing progress bar.
fn draw_demo_ui(buf: &mut [u8], pitch: usize, w: usize, h: usize, t: f32) {
    clear_xrgb8888(buf, pitch, w, h, 0x00101010);

    let cx = ((t.sin() * 0.4 + 0.5) * (w as f32 - 100.0)) as usize;
    let cy = ((t.cos() * 0.4 + 0.5) * (h as f32 - 60.0)) as usize;

    fill_rect(buf, pitch, w, h, cx, cy, 100, 60, 0x00FF8000);
    let bar = ((t * 60.0) as usize % (w - 40)).max(10);
    fill_rect(buf, pitch, w, h, 20, h - 30, bar, 10, 0x0000FF00);
}

fn fill_rect(
    buf: &mut [u8],
    pitch: usize,
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    rw: usize,
    rh: usize,
    color: u32,
) {
    let x1 = (x0 + rw).min(w);
    let y1 = (y0 + rh).min(h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let words_per_row = pitch / 4;

    for y in y0..y1 {
        let row_bytes = &mut buf[y * pitch..y * pitch + pitch];
        let row: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(row_bytes.as_mut_ptr() as *mut u32, words_per_row)
        };

        row[x0..x1].fill(color);
    }
}
