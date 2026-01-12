use std::fs::File;
use std::os::unix::io::AsRawFd;
use tokio::io::unix::AsyncFd;

use drm::buffer::Buffer as _;
use drm::control::{self, connector, crtc, Device as ControlDevice, ModeTypeFlags, PageFlipFlags};
use drm_fourcc::DrmFourcc;

use memmap2::{MmapMut, MmapOptions};
use nix::ioctl_readwrite;

// Card wrapper
#[derive(Debug)]
pub struct Card(pub File);

impl drm::Device for Card {}
impl drm::control::Device for Card {}

impl std::os::fd::AsFd for Card {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl std::os::unix::io::AsRawFd for Card {
    fn as_raw_fd(&self) -> i32 {
        self.0.as_raw_fd()
    }
}

// MAP_DUMB ioctl + mmap helper (owned mapping, no lifetimes)
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct drm_mode_map_dumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

// DRM_IOCTL_MODE_MAP_DUMB = DRM_IOWR(0xB3, struct drm_mode_map_dumb)
// https://elixir.bootlin.com/linux/v6.18.4/source/include/uapi/drm/drm_mode.h#:~:text=%2F%2A%20set,%7D%3B
ioctl_readwrite!(drm_ioctl_mode_map_dumb, b'd', 0xB3, drm_mode_map_dumb);

// Creates maps a dumb map with the appropriate *nix system function.  
fn mmap_dumb_owned(drm_file: &File, gem_handle: u32, size: usize) -> MmapMut {
    let mut req = drm_mode_map_dumb {
        handle: gem_handle,
        pad: 0,
        offset: 0,
    };

    unsafe {
        drm_ioctl_mode_map_dumb(drm_file.as_raw_fd(), &mut req).unwrap();
    }

    unsafe {
        MmapOptions::new()
            .offset(req.offset)
            .len(size)
            .map_mut(drm_file)
            .unwrap()
    }
}

/// Representation of a DumbBuffer frame which contains buffer,
/// framebuffer, pitch, and map.
pub struct Frame {
    pub buf: control::dumbbuffer::DumbBuffer,  // keep alive! (Drop likely destroys)
    pub fb: control::framebuffer::Handle,      // scanout handle
    pub pitch: usize,                          // bytes per row
    pub map: MmapMut,                          // CPU pixels
}

/// Representation of a KMS/DRM setup for a display, which contains
/// a double dumbbuffer which handles creation, flipping, and swapping.
pub struct KMSDisplay {
    pub device: AsyncFd<Card>,
    pub crtc: crtc::Handle,
    pub connector: connector::Handle,
    pub mode: control::Mode,

    pub front: Frame,
    pub back: Frame,
}

impl KMSDisplay {
    /// ## Function.
    /// Create a KMS display with two XRGB8888 dumb buffers (double buffered).
    /// Prefers HDMI if present+connected.
    /// 
    /// ## About.
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
    pub fn new(width: u32, height: u32) -> KMSDisplay {
        // Open DRM device. 
        //
        // I saw v3d (assumed) loaded on card0, 
        // whilst vc4 was loaded on card1, so I could get KMS
        // resources from it. This tries the first 16 existant
        // cards to see if they have KMS resources.
        let mut chosen: Option<Card> = None;
        for n in 0..16 {
            let path = format!("/dev/dri/card{n}");
            let f = match File::options().read(true).write(true).open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let card = Card(f);
            // KMS-capable nodes support resource_handles()
            if card.resource_handles().is_ok() {
                chosen = Some(card);
                break;
            }
        }
        let card = chosen.unwrap(); // panic if no KMS node exists

        // Open the Kernel Mode Setting resources.
        let kms_resources = card.resource_handles().unwrap();

        // Now we locate a connected connector. Since HDMI is
        // not assigned some nice unique string identifier, but
        // rather a runtime ID, we just have to find it every
        // time. We rank HDMI devices higher and attach to the
        // whatever exists.
        let (connector_handle, connector_info) = kms_resources
            .connectors()
            .iter()
            .filter_map(|&h| {
                let info = card.get_connector(h, false).ok()?;
                let ok = info.state() == connector::State::Connected && !info.modes().is_empty();
                ok.then_some((h, info))
            })
            .max_by_key(|(_h, info)| {
                let is_hdmi = matches!(
                    info.interface(),
                    connector::Interface::HDMIA | connector::Interface::HDMIB
                );
                if is_hdmi { 1 } else { 0 }
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
        let enc_info = card.get_encoder(enc_handle).unwrap();
        let crtc_handle = *kms_resources
            .filter_crtcs(enc_info.possible_crtcs())
            .get(0)
            .unwrap();

        // Create two dumb buffers (double-buffer).
        let front_buf = card
            .create_dumb_buffer((width, height), DrmFourcc::Xrgb8888, 32)
            .unwrap();
        let back_buf = card
            .create_dumb_buffer((width, height), DrmFourcc::Xrgb8888, 32)
            .unwrap();

        // Add framebuffers (depth=24, bpp=32 for XRGB8888 legacy helper).
        // Take care to note max_framebuffers=2 in /boot/firmware/config.txt.
        let front_fb = card.add_framebuffer(&front_buf, 24, 32).unwrap();
        let back_fb = card.add_framebuffer(&back_buf, 24, 32).unwrap();

        // Cache pitch before mapping (mapping holds a 
        // &mut borrow). Pitch is the number of bytes between
        // the start of one row of pixels and the start of the
        // next row in memory. You'd expect it to be 800px 
        // times 4 bytes per pixel = 3200 bytes, but there's 
        // some padding involved for alignment.
        let front_pitch = front_buf.pitch() as usize;
        let back_pitch = back_buf.pitch() as usize;

        // Calculates the size of the front and back buffers.
        let (fw, fh) = front_buf.size();
        let front_size = (front_buf.pitch() as usize) * (fh as usize);
        let (bw, bh) = back_buf.size();
        let back_size  = (back_buf.pitch() as usize) * (bh as usize);

        // Gets the handles for the dumb buffers.
        let front_handle: u32 = front_buf.handle().into();
        let back_handle: u32 = back_buf.handle().into();

        // We must mmap using the *same file descriptor* that represents the DRM device.
        // Our Card wrapper holds the underlying File as card.0, so we borrow it here.
        //
        // Important: The offset returned by MAP_DUMB is only valid for this fd.
        // Using a different File (even for the same path) will fail or map the wrong thing.
        let drm_file: &File = &card.0;

        
        // Map the dumb buffers into userspace as owned memory regions.
        // This performs the DRM_IOCTL_MODE_MAP_DUMB ioctl to get the mmap offset,
        // then calls mmap() on the DRM fd at that offset.
        // The result is a memmap2::MmapMut which:
        //   - Owns the mapping (no lifetime borrowing)
        //   - Gives us a mutable &[u8] view of the pixel memory
        //   - Is safe to store inside a struct
        //
        // This whole thing is needed to faciliate the struct form,
        // otherwise we'd use the default drm.rs stuff.
        let front_map = mmap_dumb_owned(drm_file, front_handle, front_size);
        let back_map = mmap_dumb_owned(drm_file, back_handle, back_size);

        // Package front and back frames.
        let mut front = Frame {
            buf: front_buf,
            fb: front_fb,
            pitch: front_pitch,
            map: front_map,
        };
        let back = Frame {
            buf: back_buf,
            fb: back_fb,
            pitch: back_pitch,
            map: back_map,
        };

        // Initial modeset wherein we clear front to avoid garbage.
        clear_xrgb8888(&mut front.map[..], front.pitch, width as usize, height as usize, 0x00202020);

        // Configures a display pipeline, setting a scanout engine (CRTC),
        // read pixels from a front framebuffer, drive this connector, 
        // use this display timing mode. The odd (0, 0) sets the x/y 
        // offset into the framebuffer, which tells the DMS where to begin
        // scanning. This would be changed to pan a display around a larger
        // framebuffer.
        card.set_crtc(
            crtc_handle,
            Some(front.fb),
            (0, 0),
            &[connector_handle],
            Some(mode),
        )
        .unwrap();

        // Wrap device in AsyncFd for tokio awaiting of DRM events.
        // We rename it from 'card' to 'device' because it's no longer
        // really a raw card anymore, rather an async device wrapper.
        let device = AsyncFd::new(card).unwrap();

        KMSDisplay {
            device: device,
            crtc: crtc_handle,
            connector: connector_handle,
            mode,
            front,
            back,
        }
    }

    /// Draw into back buffer then flip; caller should await flip event via your wait_for_pageflip().
    pub fn queue_flip(&self) {
        self.device
            .get_ref()
            .page_flip(self.crtc, self.back.fb, PageFlipFlags::EVENT, None)
            .unwrap();
    }

    /// Swap front/back roles (call this after the flip event completes).
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.front, &mut self.back);
    }

    pub fn copy_front_to_back(&mut self) {
        assert_eq!(self.front.map.len(), self.back.map.len());
        self.back.map.copy_from_slice(&self.front.map);
    }
}

/// Fast unsafe buffer clear.
pub fn clear_xrgb8888(buf: &mut [u8], pitch: usize, w: usize, h: usize, color: u32) {
    let words_per_row = pitch / 4;
    for y in 0..h {
        let row_bytes = &mut buf[y * pitch..y * pitch + pitch];
        let row: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(row_bytes.as_mut_ptr() as *mut u32, words_per_row)
        };
        row[..w].fill(color);
    }
}

/// Asynchronously waits for a DRM/KMS page-flip completion event. `dev` represents
/// a reference to the Tokio-wrapped DRM device (`AsyncFd<Card>`).
///
/// When a framebuffer switch is requested via `page_flip()`, the kernel does
/// not perform the swap immediately. Instead, the flip is synchronized to the
/// next vertical blank (vblank) and a `PageFlip` event is queued on the DRM file
/// descriptor once the new framebuffer is actually being scanned out.
///
/// Approximate usage:
/// ```txt
/// draw into back buffer
/// page_flip(...)
/// wait_for_pageflip(...).await   <-- wait for vblank + flip completion
/// swap front/back buffers
/// ```
///
/// Panics if `receive_events()` returns an I/O error other than
/// `ErrorKind::WouldBlock`, which indicates a serious failure of the DRM device
/// or file descriptor.
pub async fn wait_for_pageflip(dev: &AsyncFd<Card>) {
    loop {
        let mut readable = dev.readable().await.unwrap();

        let result = readable.try_io(|_| {
            // Drain events out of the device.
            let events = dev.get_ref().receive_events()?;
            for ev in events {
                if matches!(ev, drm::control::Event::PageFlip(_)) {
                    return Ok(true);
                }
            }
            Ok(false)
        });

        match result {
            Ok(Ok(true)) => return,                 // got our flip.
            Ok(Ok(false)) => continue,              // readable but not the flip we wanted.
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Ok(Err(e)) => panic!("receive_events failed: {e}"),
            Err(_would_block) => continue,          // readiness consumed; await again.
        }
    }
}

/// XRGB8888 framebuffer view (1 u32 per pixel).
/// `pitch_bytes` is the stride in *bytes* between rows.
pub struct FrameBufferData {
    pub pitch_bytes: usize,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
}

impl FrameBufferData {
    pub fn new(pitch_bytes: usize, width: usize, height: usize) -> Self {
        debug_assert!(pitch_bytes % 4 == 0, "XRGB8888 pitch must be 4-byte aligned");
        Self {pitch_bytes, width, height }
    }

    #[inline]
    pub fn words_per_row(&self) -> usize {
        self.pitch_bytes / 4
    }
}