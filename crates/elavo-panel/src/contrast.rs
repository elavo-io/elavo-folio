//! White or black for the bar's text and symbols, chosen from the wallpaper
//! strip behind each bar so the result passes WCAG AA (4.5:1).
//!
//! The bar is transparent and has an exclusive zone, so the only thing behind
//! it is the wallpaper. swaybg draws that picture scaled to cover the output,
//! centered. The same crop is reproduced here to find the rows under the bar.
//!
//! A faint gradient helps when the strip is uneven. It sits under the bar
//! and below windows, and fades out a little past the bar. Normally only white text
//! gets one, a dark tint that is hardly visible. When neither color passes
//! within that tint, the bar uses a stronger gradient: black behind white, or
//! white behind black, whichever needs less.

use std::path::Path;

use gtk::gdk_pixbuf::Pixbuf;

/// WCAG AA for normal text.
pub const AA: f64 = 4.5;
/// The strongest top-edge tint behind white text on an ordinary wallpaper.
pub const FAINT: f64 = 0.22;
/// The strongest top-edge tint when neither color passes within `FAINT`.
pub const STRONG: f64 = 0.45;
/// Height of the gradient in logical pixels. It runs on past the bar and fades
/// out over the wallpaper, so it has no edge to notice.
pub const SHADE: i32 = 84;
/// The gradient as (position down `SHADE`, share of the top alpha). It eases
/// out, losing most of its strength by just below the bar.
pub const STOPS: [(f64, f64); 3] = [(0.0, 1.0), (0.4, 0.45), (1.0, 0.0)];
/// The lowest point of the text in logical pixels from the top: 13px text
/// centered in the 28px bar. The gradient under a glyph is weakest here.
const TEXT_LOW: f64 = 20.0;
/// Decode width. Enough rows for a 28px strip, cheap for a 5K picture.
const DECODE_WIDTH: i32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    White,
    Black,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub ink: Ink,
    /// Alpha of the gradient at the top edge. Black behind white ink, white
    /// behind black ink. Zero is no gradient.
    pub tint: f64,
    /// The worst-case contrast of the ink against the tinted strip.
    pub contrast: f64,
}

impl Look {
    /// A dark desktop, or a picture that cannot be read.
    pub const DARK: Look = Look {
        ink: Ink::White,
        tint: 0.0,
        contrast: 21.0,
    };

    pub fn passes(&self) -> bool {
        self.contrast >= AA
    }
}

/// Luminance of the strip under the bar: the darkest and brightest tenth, and
/// the mean. The percentiles keep one bright cloud or dark corner under a
/// word from being averaged away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub dark: f64,
    pub mean: f64,
    pub bright: f64,
}

fn linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(linear: f64) -> f64 {
    if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// WCAG relative luminance of an sRGB pixel.
pub fn luminance(r: u8, g: u8, b: u8) -> f64 {
    let c = |v: u8| linear(f64::from(v) / 255.0);
    0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b)
}

/// WCAG contrast ratio of two luminances.
pub fn contrast(a: f64, b: f64) -> f64 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// The luminance after the toolkit paints a black or white layer of `alpha`
/// over it. Blending happens on the encoded sRGB value, so a gray of the same
/// luminance stands in for the pixel.
fn blend(luminance: f64, alpha: f64, toward_white: bool) -> f64 {
    let v = encode(luminance.clamp(0.0, 1.0));
    let v = if toward_white {
        v * (1.0 - alpha) + alpha
    } else {
        v * (1.0 - alpha)
    };
    linear(v.clamp(0.0, 1.0))
}

/// Share of the top alpha left at `pos`, from 0 at the top to 1 at the
/// bottom of the gradient.
fn share_at(pos: f64) -> f64 {
    let pos = pos.clamp(0.0, 1.0);
    STOPS.windows(2).find(|w| pos <= w[1].0).map_or(0.0, |w| {
        let ((p0, s0), (p1, s1)) = (w[0], w[1]);
        s0 + (s1 - s0) * (pos - p0) / (p1 - p0)
    })
}

/// Share of the top alpha left under the lowest part of the text.
fn at_text() -> f64 {
    share_at(TEXT_LOW / f64::from(SHADE))
}

/// The smallest alpha under the text that brings `luminance` to AA against the
/// ink, or `None` when even a full layer does not.
fn alpha_needed(luminance: f64, ink: Ink) -> Option<f64> {
    let (ink_lum, toward_white) = match ink {
        Ink::White => (1.0, false),
        Ink::Black => (0.0, true),
    };
    let ok = |a: f64| contrast(ink_lum, blend(luminance, a, toward_white)) >= AA;
    if ok(0.0) {
        return Some(0.0);
    }
    if !ok(1.0) {
        return None;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// The ink's worst-case contrast with a top-edge tint of `tint`.
fn worst(strip: Strip, ink: Ink, tint: f64) -> f64 {
    let alpha = tint * at_text();
    match ink {
        Ink::White => contrast(1.0, blend(strip.bright, alpha, false)),
        Ink::Black => contrast(0.0, blend(strip.dark, alpha, true)),
    }
}

/// White or black, and how strong a gradient the strip needs.
pub fn choose(strip: Strip) -> Look {
    let black = worst(strip, Ink::Black, 0.0);
    if black >= AA {
        return Look {
            ink: Ink::Black,
            tint: 0.0,
            contrast: black,
        };
    }

    let white_tint = alpha_needed(strip.bright, Ink::White).map(|a| a / at_text());
    if let Some(tint) = white_tint.filter(|t| *t <= FAINT) {
        return Look {
            ink: Ink::White,
            tint,
            contrast: worst(strip, Ink::White, tint),
        };
    }

    let black_tint = alpha_needed(strip.dark, Ink::Black).map(|a| a / at_text());
    let candidates = [(Ink::White, white_tint), (Ink::Black, black_tint)];
    let easiest = candidates
        .iter()
        .filter_map(|(ink, tint)| tint.map(|t| (*ink, t)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((ink, tint)) = easiest.filter(|(_, t)| *t <= STRONG) {
        return Look {
            ink,
            tint,
            contrast: worst(strip, ink, tint),
        };
    }

    // Nothing within reach. Keep whichever ink does better at the cap.
    let white = worst(strip, Ink::White, STRONG);
    let black = worst(strip, Ink::Black, STRONG);
    if white >= black {
        Look {
            ink: Ink::White,
            tint: STRONG,
            contrast: white,
        }
    } else {
        Look {
            ink: Ink::Black,
            tint: STRONG,
            contrast: black,
        }
    }
}

/// The picture rectangle under the bar, in picture pixels: `(x0, y0, x1, y1)`.
/// `width` and `height` are the output's size, `bar` the bar's height, all in
/// the same unit.
pub fn strip_rect(
    image_w: f64,
    image_h: f64,
    width: f64,
    height: f64,
    bar: f64,
) -> (f64, f64, f64, f64) {
    let scale = (width / image_w).max(height / image_h);
    let x_off = (width - image_w * scale) / 2.0;
    let y_off = (height - image_h * scale) / 2.0;
    let x0 = (-x_off / scale).clamp(0.0, image_w);
    let x1 = ((width - x_off) / scale).clamp(0.0, image_w);
    let y0 = (-y_off / scale).clamp(0.0, image_h);
    let y1 = ((bar - y_off) / scale).clamp(0.0, image_h);
    (x0, y0, x1, y1)
}

/// Mean and percentiles of a set of luminances.
pub fn measure(values: &mut [f64]) -> Option<Strip> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    Some(Strip {
        dark: at(0.10),
        mean,
        bright: at(0.90),
    })
}

/// Read the picture and measure the strip under a bar of `bar` logical pixels
/// on an output of `width` by `height` logical pixels.
pub fn sample(path: &Path, width: i32, height: i32, bar: i32) -> Result<Strip, String> {
    let pixbuf =
        Pixbuf::from_file_at_scale(path, DECODE_WIDTH, -1, true).map_err(|e| e.to_string())?;
    let (iw, ih) = (pixbuf.width(), pixbuf.height());
    let (x0, y0, x1, y1) = strip_rect(
        f64::from(iw),
        f64::from(ih),
        f64::from(width),
        f64::from(height),
        f64::from(bar),
    );
    let (x0, x1) = (x0.floor() as i32, (x1.ceil() as i32).min(iw));
    let (y0, y1) = (
        y0.floor() as i32,
        (y1.ceil() as i32).clamp(y0 as i32 + 1, ih),
    );
    let bytes = pixbuf.read_pixel_bytes();
    let stride = pixbuf.rowstride() as usize;
    let channels = pixbuf.n_channels() as usize;
    let mut values = Vec::with_capacity(((x1 - x0) * (y1 - y0)).max(0) as usize);
    for y in y0..y1 {
        let row = y as usize * stride;
        for x in x0..x1 {
            let i = row + x as usize * channels;
            if let Some(px) = bytes.get(i..i + 3) {
                values.push(luminance(px[0], px[1], px[2]));
            }
        }
    }
    measure(&mut values).ok_or_else(|| "the strip under the bar is empty".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(l: f64) -> Strip {
        Strip {
            dark: l,
            mean: l,
            bright: l,
        }
    }

    #[test]
    fn wcag_reference_values() {
        assert!((contrast(1.0, 0.0) - 21.0).abs() < 1e-9);
        assert!((luminance(255, 255, 255) - 1.0).abs() < 1e-9);
        assert_eq!(luminance(0, 0, 0), 0.0);
        // #767676 on white is the classic 4.54:1.
        let gray = luminance(0x76, 0x76, 0x76);
        assert!((contrast(1.0, gray) - 4.54).abs() < 0.01);
    }

    #[test]
    fn white_strip_takes_black_ink() {
        let look = choose(flat(1.0));
        assert_eq!(look.ink, Ink::Black);
        assert_eq!(look.tint, 0.0);
        assert!(look.passes());
    }

    #[test]
    fn black_strip_takes_white_ink_without_a_gradient() {
        let look = choose(flat(0.0));
        assert_eq!(look.ink, Ink::White);
        assert_eq!(look.tint, 0.0);
        assert!(look.passes());
    }

    #[test]
    fn mid_gray_passes_with_one_of_the_two() {
        let look = choose(flat(luminance(0x80, 0x80, 0x80)));
        assert!(look.passes());
        assert!(look.tint <= FAINT);
    }

    #[test]
    fn a_bright_patch_on_a_dark_strip_gets_a_faint_black_gradient() {
        let look = choose(Strip {
            dark: 0.02,
            mean: 0.08,
            bright: 0.24,
        });
        assert_eq!(look.ink, Ink::White);
        assert!(look.tint > 0.0 && look.tint <= FAINT, "{look:?}");
        assert!(look.passes());
    }

    #[test]
    fn a_split_strip_takes_the_smaller_strong_gradient() {
        let look = choose(Strip {
            dark: 0.06,
            mean: 0.4,
            bright: 0.85,
        });
        assert!(look.tint > FAINT && look.tint <= STRONG, "{look:?}");
        assert!(look.passes(), "{look:?}");
        let other = match look.ink {
            Ink::White => Ink::Black,
            Ink::Black => Ink::White,
        };
        let other_needs = match other {
            Ink::White => alpha_needed(0.85, Ink::White),
            Ink::Black => alpha_needed(0.06, Ink::Black),
        }
        .map(|a| a / at_text());
        assert!(other_needs.is_none_or(|t| t >= look.tint));
    }

    #[test]
    fn an_impossible_strip_keeps_the_better_ink_at_the_cap() {
        let look = choose(Strip {
            dark: 0.0,
            mean: 0.5,
            bright: 1.0,
        });
        assert_eq!(look.tint, STRONG);
        assert!(!look.passes());
    }

    #[test]
    fn cover_crop_on_a_16_10_output_with_a_16_9_picture() {
        // 5120x2880 on 1920x1200: height decides the scale, the sides are cut.
        let (x0, y0, x1, y1) = strip_rect(5120.0, 2880.0, 1920.0, 1200.0, 28.0);
        let scale = 1200.0 / 2880.0;
        let cut = (5120.0 * scale - 1920.0) / 2.0 / scale;
        assert!((x0 - cut).abs() < 1e-6);
        assert!((x1 - (5120.0 - cut)).abs() < 1e-6);
        assert_eq!(y0, 0.0);
        assert!((y1 - 28.0 / scale).abs() < 1e-6);
    }

    #[test]
    fn cover_crop_on_an_ultrawide_output_cuts_the_top() {
        let (x0, y0, x1, y1) = strip_rect(1600.0, 900.0, 2560.0, 1080.0, 28.0);
        assert_eq!((x0, x1), (0.0, 1600.0));
        assert!(y0 > 0.0);
        assert!(y1 > y0);
    }

    #[test]
    fn the_gradient_eases_out_to_nothing() {
        assert_eq!(share_at(0.0), 1.0);
        assert!((share_at(0.4) - 0.45).abs() < 1e-9);
        assert!(share_at(1.0).abs() < 1e-9);
        let under_text = at_text();
        assert!(under_text > 0.45 && under_text < 1.0, "{under_text}");
    }

    #[test]
    fn percentiles() {
        let mut values: Vec<f64> = (0..=100).map(|v| f64::from(v) / 100.0).collect();
        let strip = measure(&mut values).unwrap();
        assert!((strip.dark - 0.10).abs() < 1e-9);
        assert!((strip.bright - 0.90).abs() < 1e-9);
        assert!((strip.mean - 0.5).abs() < 1e-9);
    }
}
