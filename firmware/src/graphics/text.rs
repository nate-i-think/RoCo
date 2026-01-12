use ab_glyph::{Font, FontArc, OutlinedGlyph, PxScale, ScaleFont};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use crate::graphics::display::FrameBufferData;

/// Compile-time embedded font bytes.
/// Path is relative to project root.
pub static ROBOTO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.compile_time_data/Roboto-Regular.ttf"
));

pub static ROBOTO_MONO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.compile_time_data/RobotoMono-Regular.ttf"
));

const MAGIC: &[u8; 8] = b"ROCOATL1"; // 8 bytes
const VERSION: u32 = 1;

#[derive(Debug)]
pub enum AtlasError {
    Io(std::io::Error),
    InvalidFormat(&'static str),
    Unsupported(&'static str),
}

impl From<std::io::Error> for AtlasError {
    fn from(e: std::io::Error) -> Self {
        AtlasError::Io(e)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RectU16 {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct GlyphInfo {
    // If rect.w==0, treat as missing / no pixels (space etc. still has advance).
    rect: RectU16,
    min_x: i16,
    min_y: i16,
    advance_x: i16, // pixels, rounded
    _pad: i16,
}

#[derive(Clone, Copy, Debug, Default)]
struct GlyphEntryDisk {
    ch: u32,
    rect: RectU16,
    min_x: i16,
    min_y: i16,
    advance_x: i16,
}

/// Hashing via Fowler–Noll–Vo
#[inline(always)]
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[inline(always)]
fn charset_hash(chars: &[char]) -> u64 {
    // Hash u32 scalar values in LE.
    let mut tmp = [0u8; 4];
    let mut h: u64 = 0xcbf29ce484222325;
    for &ch in chars {
        tmp.copy_from_slice(&(ch as u32).to_le_bytes());
        for &b in &tmp {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// Fast approximate (x / 255) for x in [0..(255*255)]
#[inline(always)]
fn div255(x: u32) -> u32 {
    // (x + 128 + ((x + 128) >> 8)) >> 8
    let t = x + 128;
    (t + (t >> 8)) >> 8
}

#[inline(always)]
fn mul8(a: u32, b: u32) -> u32 {
    // a,b in 0..255
    div255(a * b)
}

/// A cached, pre-rasterized alpha atlas + glyph metrics.
/// Runtime draw does NOT require keeping the Font object.
pub struct FontAtlas {
    pub font_name: String,
    pub px: u16,

    atlas_w: u32,
    atlas_h: u32,
    alpha: Vec<u8>,

    // Fast path for ASCII (0..127)
    ascii: [GlyphInfo; 128],
    // Fallback for non-ASCII (optional)
    other: HashMap<char, GlyphInfo>,

    // Metrics (pixels)
    pub ascent: i16,
    pub descent: i16,
    pub line_gap: i16,
    pub line_height: i16,
}

impl FontAtlas {
    /// Convenience: baseline_y = top_y + ascent
    #[inline]
    pub fn baseline_from_top(&self, top_y: isize) -> isize {
        top_y + self.ascent as isize
    }

    /// Default charset: printable ASCII.
    pub fn ascii_printable() -> Vec<char> {
        (32u8..=126u8).map(|b| b as char).collect()
    }

    /// Load from cache if valid; otherwise build from embedded bytes and write cache.
    pub fn load_or_build_cached(
        font_name: &str,
        font_bytes: &'static [u8],
        px: u16,
        charset: &[char],
        cache_root: impl AsRef<Path>,
        // atlas size for the first build; 512 is fine for one size + ASCII.
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<Self, AtlasError> {
        let cache_dir = cache_root.as_ref().join(font_name);
        let cache_path = cache_dir.join(format!("atlas_px{}.bin", px));

        // Try load
        if let Ok(atlas) = Self::load_from_file(&cache_path, font_name, font_bytes, px, charset) {
            return Ok(atlas);
        }

        // Build + write
        let atlas = Self::build_from_font(font_name, font_bytes, px, charset, atlas_w, atlas_h)?;

        fs::create_dir_all(&cache_dir)?;
        Self::write_atomic(&cache_path, &atlas, font_bytes, charset)?;

        Ok(atlas)
    }

    /// Build from a font (rasterizes glyphs into an alpha atlas).
    fn build_from_font(
        font_name: &str,
        font_bytes: &'static [u8],
        px: u16,
        charset: &[char],
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<Self, AtlasError> {
        let font = FontArc::try_from_slice(font_bytes).map_err(|_| AtlasError::InvalidFormat("bad font bytes"))?;
        let scale = PxScale::from(px as f32);
        let scaled = font.as_scaled(scale);

        let ascent = scaled.ascent().round() as i16;
        let descent = scaled.descent().abs().round() as i16;
        let line_gap = scaled.line_gap().round() as i16;
        let line_height = ascent + descent + line_gap;

        let mut alpha = vec![0u8; (atlas_w * atlas_h) as usize];

        let mut ascii = [GlyphInfo::default(); 128];
        let mut other = HashMap::<char, GlyphInfo>::new();

        // Very simple shelf packing
        let pad: u32 = 1;
        let mut cx: u32 = pad;
        let mut cy: u32 = pad;
        let mut row_h: u32 = 0;

        for &ch in charset {
            let gid = font.glyph_id(ch);
            let advance_x = scaled.h_advance(gid).round() as i16;

            let g = gid.with_scale(scale);
            let outlined: Option<OutlinedGlyph> = font.outline_glyph(g);

            let info = if let Some(og) = outlined {
                let pb = og.px_bounds();

                let gw = pb.width().ceil().max(0.0) as u32;
                let gh = pb.height().ceil().max(0.0) as u32;

                let min_x = pb.min.x.floor() as i16;
                let min_y = pb.min.y.floor() as i16;

                if gw == 0 || gh == 0 {
                    GlyphInfo {
                        rect: RectU16::default(),
                        min_x,
                        min_y,
                        advance_x,
                        _pad: 0,
                    }
                } else {
                    if cx + gw + pad > atlas_w {
                        cx = pad;
                        cy = cy + row_h + pad;
                        row_h = 0;
                    }
                    if cy + gh + pad > atlas_h {
                        return Err(AtlasError::Unsupported("atlas too small; increase atlas_w/atlas_h"));
                    }

                    // Rasterize into temp
                    let mut gb = vec![0u8; (gw * gh) as usize];
                    og.draw(|x: u32, y: u32, v: f32| {
                        let a = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        let idx = (y as u32 * gw + x as u32) as usize;
                        // max is safe
                        let cur = gb[idx];
                        gb[idx] = if a > cur { a } else { cur };
                    });

                    // Copy into atlas
                    for yy in 0..gh {
                        let src_row = (yy * gw) as usize;
                        let dst_row = ((cy + yy) * atlas_w + cx) as usize;
                        alpha[dst_row..dst_row + gw as usize]
                            .copy_from_slice(&gb[src_row..src_row + gw as usize]);
                    }

                    row_h = row_h.max(gh);

                    let rect = RectU16 { x: cx as u16, y: cy as u16, w: gw as u16, h: gh as u16 };
                    cx += gw + pad;

                    GlyphInfo {
                        rect,
                        min_x,
                        min_y,
                        advance_x,
                        _pad: 0,
                    }
                }
            } else {
                // No outline (space etc.)
                GlyphInfo {
                    rect: RectU16::default(),
                    min_x: 0,
                    min_y: 0,
                    advance_x,
                    _pad: 0,
                }
            };

            let code = ch as u32;
            if code < 128 {
                ascii[code as usize] = info;
            } else {
                other.insert(ch, info);
            }
        }

        Ok(Self {
            font_name: font_name.to_string(),
            px,
            atlas_w,
            atlas_h,
            alpha,
            ascii,
            other,
            ascent,
            descent,
            line_gap,
            line_height,
        })
    }

    /// Draw with alpha blending against the EXISTING framebuffer pixels.
    /// Buffer is XRGB8888 (u32 per pixel) presented as `&mut [u8]` with `pitch` bytes per row.
    pub fn draw_text_blend_xrgb8888(
        &self,
        buf: &mut [u8],
        pitch: usize,
        w: usize,
        h: usize,
        x: isize,
        baseline_y: isize,
        fg: u32, // 0x00RRGGBB
        text: &str,
    ) {
        debug_assert!(pitch % 4 == 0);
        let words_per_row = pitch / 4;
        debug_assert!(w <= words_per_row);

        let mut pen_x = x as i32;
        let base_y = baseline_y as i32;

        for ch in text.chars() {
            let info = self.lookup(ch);
            // Missing glyph fallback: try '?'
            let info = if info.rect.w == 0 && info.advance_x == 0 {
                self.lookup('?')
            } else {
                info
            };

            if info.rect.w != 0 && info.rect.h != 0 {
                let dst_x0 = pen_x + info.min_x as i32;
                let dst_y0 = base_y + info.min_y as i32;
                self.blit_alpha_over_xrgb8888(
                    buf, pitch, w, h,
                    dst_x0, dst_y0,
                    info.rect,
                    fg,
                );
            }

            pen_x += info.advance_x as i32;
        }
    }

    /// Draw text on a SOLID background color (fast path):
    /// - measure approximate bounds
    /// - fill one bg rect
    /// - draw glyphs blended over that bg
    /// NOTE: `fg` and `bg` are in 0x00RRGGBB
    pub fn draw_text_on_solid_bg_xrgb8888(
        &self,
        buf: &mut [u8],
        pitch: usize,
        w: usize,
        h: usize,
        x: isize,
        baseline_y: isize,
        fg: u32, // 0x00RRGGBB
        bg: u32, // 0x00RRGGBB
        text: &str,
    ) {
        // Fill background behind the text bounds, then draw normally.
        if let Some((rx, ry, rw, rh)) = self.measure_text_bounds(x, baseline_y, text) {
            draw_rect_xrgb8888(buf, pitch, w, h, rx, ry, rw, rh, bg);
        }
        self.draw_text_blend_xrgb8888(buf, pitch, w, h, x, baseline_y, fg, text);
    }

    /// Bounding rect of the *union of glyph bitmap rectangles* for the string.
    /// Returns (x, y, w, h) in framebuffer coords.
    fn measure_text_bounds(
        &self,
        x: isize,
        baseline_y: isize,
        text: &str,
    ) -> Option<(isize, isize, usize, usize)> {
        let mut pen_x = x as i32;
        let base_y = baseline_y as i32;

        let mut min_x: i32 = i32::MAX;
        let mut min_y: i32 = i32::MAX;
        let mut max_x: i32 = i32::MIN;
        let mut max_y: i32 = i32::MIN;

        let mut any = false;

        for ch in text.chars() {
            let info = self.lookup(ch);
            let info = if info.rect.w == 0 && info.advance_x == 0 { self.lookup('?') } else { info };

            if info.rect.w != 0 && info.rect.h != 0 {
                let gx0 = pen_x + info.min_x as i32;
                let gy0 = base_y + info.min_y as i32;
                let gx1 = gx0 + info.rect.w as i32;
                let gy1 = gy0 + info.rect.h as i32;

                min_x = min_x.min(gx0);
                min_y = min_y.min(gy0);
                max_x = max_x.max(gx1);
                max_y = max_y.max(gy1);

                any = true;
            }

            pen_x += info.advance_x as i32;
        }

        if !any || min_x >= max_x || min_y >= max_y {
            return None;
        }

        let rx = min_x as isize;
        let ry = min_y as isize;
        let rw = (max_x - min_x) as usize;
        let rh = (max_y - min_y) as usize;

        Some((rx, ry, rw, rh))
    }

    #[inline(always)]
    fn lookup(&self, ch: char) -> GlyphInfo {
        let code = ch as u32;
        if code < 128 {
            self.ascii[code as usize]
        } else {
            self.other.get(&ch).copied().unwrap_or_default()
        }
    }

    #[inline(always)]
    fn blit_alpha_over_xrgb8888(
        &self,
        buf: &mut [u8],
        pitch: usize,
        w: usize,
        h: usize,
        dst_x0: i32,
        dst_y0: i32,
        src: RectU16,
        fg: u32,
    ) {
        let words_per_row = pitch / 4;

        let src_x0 = src.x as i32;
        let src_y0 = src.y as i32;
        let gw = src.w as i32;
        let gh = src.h as i32;

        // clip to framebuffer
        let x0 = dst_x0.max(0);
        let y0 = dst_y0.max(0);
        let x1 = (dst_x0 + gw).min(w as i32);
        let y1 = (dst_y0 + gh).min(h as i32);
        if x0 >= x1 || y0 >= y1 { return; }

        let sr = ((fg >> 16) & 0xFF) as u32;
        let sg = ((fg >> 8) & 0xFF) as u32;
        let sb = (fg & 0xFF) as u32;

        for yy in y0..y1 {
            let row_bytes = &mut buf[yy as usize * pitch..yy as usize * pitch + pitch];
            let row: &mut [u32] = unsafe {
                std::slice::from_raw_parts_mut(row_bytes.as_mut_ptr() as *mut u32, words_per_row)
            };

            let sy = src_y0 + (yy - dst_y0);
            let src_row = (sy as u32 * self.atlas_w) as usize;

            for xx in x0..x1 {
                let sx = src_x0 + (xx - dst_x0);
                let a = self.alpha[src_row + sx as usize] as u32;
                if a == 0 { continue; }

                let idx = xx as usize;

                if a == 255 {
                    row[idx] = fg;
                    continue;
                }

                let inv = 255 - a;
                let dst = row[idx];
                let dr = ((dst >> 16) & 0xFF) as u32;
                let dg = ((dst >> 8) & 0xFF) as u32;
                let db = (dst & 0xFF) as u32;

                // channel = (src*a + dst*(255-a))/255  using fast div255
                let r = mul8(sr, a) + mul8(dr, inv);
                let g = mul8(sg, a) + mul8(dg, inv);
                let b = mul8(sb, a) + mul8(db, inv);

                row[idx] = (r << 16) | (g << 8) | b;
            }
        }
    }

    // ---------------- Disk I/O ----------------

    fn write_atomic(path: &Path, atlas: &FontAtlas, font_bytes: &[u8], charset: &[char]) -> Result<(), AtlasError> {
        let tmp = path.with_extension("tmp");

        let mut out = Vec::<u8>::with_capacity(
            128 + (atlas.alpha.len()) + (charset.len() * 32)
        );

        // Header
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());

        let font_hash = fnv1a64(font_bytes);
        let cs_hash = charset_hash(charset);

        out.extend_from_slice(&font_hash.to_le_bytes());
        out.extend_from_slice(&cs_hash.to_le_bytes());

        out.extend_from_slice(&(atlas.px as u16).to_le_bytes());
        out.extend_from_slice(&(atlas.atlas_w as u32).to_le_bytes());
        out.extend_from_slice(&(atlas.atlas_h as u32).to_le_bytes());

        out.extend_from_slice(&atlas.ascent.to_le_bytes());
        out.extend_from_slice(&atlas.descent.to_le_bytes());
        out.extend_from_slice(&atlas.line_gap.to_le_bytes());
        out.extend_from_slice(&atlas.line_height.to_le_bytes());

        let glyph_count = charset.len() as u32;
        out.extend_from_slice(&glyph_count.to_le_bytes());

        // Glyph entries
        for &ch in charset {
            let info = atlas.lookup(ch);
            let e = GlyphEntryDisk {
                ch: ch as u32,
                rect: info.rect,
                min_x: info.min_x,
                min_y: info.min_y,
                advance_x: info.advance_x,
            };
            write_glyph_entry(&mut out, &e);
        }

        // Alpha
        out.extend_from_slice(&atlas.alpha);

        // Write tmp then rename
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_all()?;
        }
        fs::rename(tmp, path)?;

        Ok(())
    }

    fn load_from_file(
        path: &Path,
        expected_font_name: &str,
        font_bytes: &[u8],
        px: u16,
        charset: &[char],
    ) -> Result<Self, AtlasError> {
        let mut f = fs::File::open(path)?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;

        let mut cur = 0usize;

        let magic = read_exact(&data, &mut cur, 8)?;
        if magic != MAGIC {
            return Err(AtlasError::InvalidFormat("bad magic"));
        }

        let ver = read_u32(&data, &mut cur)?;
        if ver != VERSION {
            return Err(AtlasError::InvalidFormat("version mismatch"));
        }

        let font_hash_file = read_u64(&data, &mut cur)?;
        let cs_hash_file = read_u64(&data, &mut cur)?;

        let font_hash_now = fnv1a64(font_bytes);
        let cs_hash_now = charset_hash(charset);

        if font_hash_file != font_hash_now {
            return Err(AtlasError::InvalidFormat("font hash mismatch"));
        }
        if cs_hash_file != cs_hash_now {
            return Err(AtlasError::InvalidFormat("charset hash mismatch"));
        }

        let px_file = read_u16(&data, &mut cur)?;
        if px_file != px {
            return Err(AtlasError::InvalidFormat("px mismatch"));
        }

        let atlas_w = read_u32(&data, &mut cur)?;
        let atlas_h = read_u32(&data, &mut cur)?;

        let ascent = read_i16(&data, &mut cur)?;
        let descent = read_i16(&data, &mut cur)?;
        let line_gap = read_i16(&data, &mut cur)?;
        let line_height = read_i16(&data, &mut cur)?;

        let glyph_count = read_u32(&data, &mut cur)? as usize;
        if glyph_count != charset.len() {
            return Err(AtlasError::InvalidFormat("glyph_count mismatch"));
        }

        let mut ascii = [GlyphInfo::default(); 128];
        let mut other = HashMap::<char, GlyphInfo>::new();

        for _ in 0..glyph_count {
            let e = read_glyph_entry(&data, &mut cur)?;
            let ch = char::from_u32(e.ch).ok_or(AtlasError::InvalidFormat("invalid char"))?;

            let info = GlyphInfo {
                rect: e.rect,
                min_x: e.min_x,
                min_y: e.min_y,
                advance_x: e.advance_x,
                _pad: 0,
            };

            if (e.ch) < 128 {
                ascii[e.ch as usize] = info;
            } else {
                other.insert(ch, info);
            }
        }

        let alpha_len = (atlas_w as usize) * (atlas_h as usize);
        let alpha = read_exact(&data, &mut cur, alpha_len)?.to_vec();

        Ok(Self {
            font_name: expected_font_name.to_string(),
            px: px_file,
            atlas_w,
            atlas_h,
            alpha,
            ascii,
            other,
            ascent,
            descent,
            line_gap,
            line_height,
        })
    }
}

// ----- Binary helpers -----

#[inline(always)]
fn read_exact<'a>(data: &'a [u8], cur: &mut usize, n: usize) -> Result<&'a [u8], AtlasError> {
    if *cur + n > data.len() {
        return Err(AtlasError::InvalidFormat("unexpected EOF"));
    }
    let s = &data[*cur..*cur + n];
    *cur += n;
    Ok(s)
}

#[inline(always)]
fn read_u16(data: &[u8], cur: &mut usize) -> Result<u16, AtlasError> {
    let s = read_exact(data, cur, 2)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

#[inline(always)]
fn read_i16(data: &[u8], cur: &mut usize) -> Result<i16, AtlasError> {
    let s = read_exact(data, cur, 2)?;
    Ok(i16::from_le_bytes([s[0], s[1]]))
}

#[inline(always)]
fn read_u32(data: &[u8], cur: &mut usize) -> Result<u32, AtlasError> {
    let s = read_exact(data, cur, 4)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline(always)]
fn read_u64(data: &[u8], cur: &mut usize) -> Result<u64, AtlasError> {
    let s = read_exact(data, cur, 8)?;
    Ok(u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

#[inline(always)]
fn write_glyph_entry(out: &mut Vec<u8>, e: &GlyphEntryDisk) {
    out.extend_from_slice(&e.ch.to_le_bytes());
    out.extend_from_slice(&e.rect.x.to_le_bytes());
    out.extend_from_slice(&e.rect.y.to_le_bytes());
    out.extend_from_slice(&e.rect.w.to_le_bytes());
    out.extend_from_slice(&e.rect.h.to_le_bytes());
    out.extend_from_slice(&e.min_x.to_le_bytes());
    out.extend_from_slice(&e.min_y.to_le_bytes());
    out.extend_from_slice(&e.advance_x.to_le_bytes());
}

#[inline(always)]
fn read_glyph_entry(data: &[u8], cur: &mut usize) -> Result<GlyphEntryDisk, AtlasError> {
    let ch = read_u32(data, cur)?;
    let x = read_u16(data, cur)?;
    let y = read_u16(data, cur)?;
    let w = read_u16(data, cur)?;
    let h = read_u16(data, cur)?;
    let min_x = read_i16(data, cur)?;
    let min_y = read_i16(data, cur)?;
    let advance_x = read_i16(data, cur)?;
    Ok(GlyphEntryDisk {
        ch,
        rect: RectU16 { x, y, w, h },
        min_x,
        min_y,
        advance_x,
    })
}

/// Rect fill helper
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


// -------------------------------------------------------------------------------------------------
// ColoredText: draw multi-colored strings as a sequence of spans.
// -------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ColoredText {
    spans: Vec<ColoredSpan>,
}

#[derive(Clone, Debug)]
struct ColoredSpan {
    text: String,
    color: u32, // 0x00RRGGBB
}

impl ColoredText {
    #[inline]
    pub fn new() -> Self {
        Self { spans: Vec::new() }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Append `text` with `color` (0x00RRGGBB).
    /// Coalesces with the previous span if the color matches.
    #[inline]
    pub fn push(&mut self, text: impl AsRef<str>, color: u32) {
        let s = text.as_ref();
        if s.is_empty() {
            return;
        }

        if let Some(last) = self.spans.last_mut() {
            if last.color == color {
                last.text.push_str(s);
                return;
            }
        }

        self.spans.push(ColoredSpan {
            text: s.to_owned(),
            color,
        });
    }

    /// Total advance (in pixels) for all spans, using this atlas's cached metrics.
    #[inline]
    pub fn width_px(&self, atlas: &FontAtlas) -> i32 {
        let mut w = 0i32;
        for span in &self.spans {
            w += advance_px(atlas, &span.text);
        }
        w
    }

    /// Draw multi-colored text at `(x, baseline_y)`, advancing the pen as each span is drawn.
    /// Colors are 0x00RRGGBB and are alpha-blended over the existing framebuffer pixels.
    #[inline]
    pub fn draw_blend_xrgb8888(
        &self,
        atlas: &FontAtlas,
        fb: &mut [u8],
        fb_data: &FrameBufferData,
        x: isize,
        baseline_y: isize,
    ) {
        let mut pen_x = x;
        for span in &self.spans {
            atlas.draw_text_blend_xrgb8888(
                fb,
                fb_data.pitch_bytes,
                fb_data.width,
                fb_data.height,
                pen_x,
                baseline_y,
                span.color,
                &span.text,
            );
            pen_x += advance_px(atlas, &span.text) as isize;
        }
    }
}

#[inline(always)]
fn advance_px(atlas: &FontAtlas, text: &str) -> i32 {
    let mut pen_x = 0i32;

    for ch in text.chars() {
        let info = atlas.lookup(ch);
        // Mirror draw_* fallback behavior: if missing, try '?'
        let info = if info.rect.w == 0 && info.advance_x == 0 {
            atlas.lookup('?')
        } else {
            info
        };
        pen_x += info.advance_x as i32;
    }

    pen_x
}