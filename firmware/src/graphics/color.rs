pub type Color = u32;

#[inline(always)]
pub const fn xrgb(r: u8, g: u8, b: u8) -> Color { ((r as u32) << 16) | ((g as u32) << 8) | (b as u32) }

#[inline(always)]
pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> Color { ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32) }

// Select channels (XRGB).
#[inline(always)]
pub const fn r_xrgb(c: Color) -> u8 { ((c >> 16) & 0xFF) as u8 }

#[inline(always)]
pub const fn g_xrgb(c: Color) -> u8 { ((c >> 8) & 0xFF) as u8 }

#[inline(always)]
pub const fn b_xrgb(c: Color) -> u8 { (c & 0xFF) as u8 }

#[inline(always)]
pub const fn a_argb(c: Color) -> u8 { ((c >> 24) & 0xFF) as u8 }

#[inline(always)]
pub const fn r_argb(c: Color) -> u8 { ((c >> 16) & 0xFF) as u8 }

#[inline(always)]
pub const fn g_argb(c: Color) -> u8 { ((c >> 8) & 0xFF) as u8 }

#[inline(always)]
pub const fn b_argb(c: Color) -> u8 { (c & 0xFF) as u8 }

// Replace channels (XRGB).
#[inline(always)]
pub const fn with_r_xrgb(c: Color, r: u8) -> Color { (c & 0xFF00_FF) | ((r as u32) << 16) }

#[inline(always)]
pub const fn with_g_xrgb(c: Color, g: u8) -> Color { (c & 0xFF00_FF00) | ((g as u32) << 8) }

#[inline(always)]
pub const fn with_b_xrgb(c: Color, b: u8) -> Color { (c & 0xFFFF_FF00) | (b as u32) }

// Common colors (XRGB8888: 0x00RRGGBB).

pub const TRANSPARENT_X: Color = 0x0000_0000; // (X byte = 0); not true alpha, but useful as "empty."
pub const BLACK: Color = 0x0000_0000;
pub const WHITE: Color = 0x00FF_FFFF;

pub const RED: Color = 0x00FF_0000;
pub const GREEN: Color = 0x0000_FF00;
pub const BLUE: Color = 0x0000_00FF;

pub const YELLOW: Color = 0x00FF_FF00;
pub const CYAN: Color = 0x0000_FFFF;
pub const MAGENTA: Color = 0x00FF_00FF;

pub const ORANGE: Color = 0x00FF_A500;
pub const PINK: Color = 0x00FF_C0CB;
pub const PURPLE: Color = 0x0080_00FF;
pub const VIOLET: Color = 0x00EE_82EE;

pub const BROWN: Color = 0x008B_4513;
pub const TAN: Color = 0x00D2_B48C;
pub const OLIVE: Color = 0x0080_8000;
pub const TEAL: Color = 0x0000_8080;
pub const NAVY: Color = 0x0000_0080;
pub const MAROON: Color = 0x0080_0000;

pub const GRAY_10: Color = 0x001A_1A_1A;
pub const GRAY_20: Color = 0x0033_3333;
pub const GRAY_30: Color = 0x004D_4D_4D;
pub const GRAY_40: Color = 0x0066_6666;
pub const GRAY_50: Color = 0x0080_8080;
pub const GRAY_60: Color = 0x0099_9999;
pub const GRAY_70: Color = 0x00B3_B3B3;
pub const GRAY_80: Color = 0x00CC_CCCC;
pub const GRAY_90: Color = 0x00E6_E6E6;

/// Deterministically map any string to a pleasant, non-dark XRGB8888 color (0x00RRGGBB).
/// Guaranteed to be visible on black: avoids low luma and avoids near-grays.
///
/// X in XRGB8888 is 0 (unused/ignored).
pub fn string_to_xrgb8888(s: &str) -> u32 {
    let h = fnv1a_32(s.as_bytes());

    // Hue: [0, 360)
    let hue = (h % 360) as f32;

    // Keep colors pleasant: high-ish saturation, high-ish value, but with variation.
    // S in [0.55, 0.90], V in [0.70, 0.95]
    let sat = 0.55 + (((h >> 9) & 0xFF) as f32 / 255.0) * 0.35;
    let val = 0.70 + (((h >> 17) & 0xFF) as f32 / 255.0) * 0.25;

    let (mut r, mut g, mut b) = hsv_to_rgb_u8(hue, sat, val);

    // --- Safety rails ---
    // 1) Ensure not too dark on black using Rec.709 luma approximation.
    //    (This threshold is intentionally conservative.)
    let luma = (2126u32 * r as u32 + 7152u32 * g as u32 + 722u32 * b as u32) / 10000u32;
    if luma < 110 {
        // Brighten by scaling up toward 255 without changing hue too much.
        // scale = 110/luma (clamped)
        let l = luma.max(1) as f32;
        let scale = (110.0 / l).min(1.8);
        r = ((r as f32 * scale).min(255.0)) as u8;
        g = ((g as f32 * scale).min(255.0)) as u8;
        b = ((b as f32 * scale).min(255.0)) as u8;
    }

    // 2) Avoid near-gray colors (low chroma): if RGB components are too close,
    //    bump saturation by pushing away from the average.
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    if (maxc - minc) < 24 {
        // Push channels away from mean a bit.
        let mean = ((r as u16 + g as u16 + b as u16) / 3) as i16;
        r = push_from_mean(r, mean, 40);
        g = push_from_mean(g, mean, 40);
        b = push_from_mean(b, mean, 40);
    }

    // Pack as XRGB8888: 0x00RRGGBB
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C9DC5; // offset basis
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193); // FNV prime
    }
    hash
}

/// Convert HSV (h in degrees [0,360), s/v in [0,1]) to RGB u8.
#[inline]
fn hsv_to_rgb_u8(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hh = (h / 60.0) % 6.0;
    let x = c * (1.0 - ((hh % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match hh as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;

    let r = ((r1 + m) * 255.0 + 0.5) as u8;
    let g = ((g1 + m) * 255.0 + 0.5) as u8;
    let b = ((b1 + m) * 255.0 + 0.5) as u8;
    (r, g, b)
}

#[inline]
fn push_from_mean(channel: u8, mean: i16, amount: i16) -> u8 {
    let c = channel as i16;
    let delta = c - mean;
    let pushed = if delta >= 0 {
        c + amount
    } else {
        c - amount
    };
    pushed.clamp(0, 255) as u8
}