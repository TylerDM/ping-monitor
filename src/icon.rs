use ab_glyph::{Font, FontVec, OutlinedGlyph, PxScale, Rect, ScaleFont, point};
use std::ffi::c_void;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};
use windows::core::w;

/// Text colour for a severity from 0 (fine) to 1 (bad): the taskbar's own text
/// colour, blending towards red. Both ends are Windows 11's taskbar text and
/// critical-red colours for the given theme.
pub fn color(severity: f32, light_taskbar: bool) -> [u8; 3] {
    let (normal, red) = if light_taskbar {
        ([0x1B, 0x1B, 0x1B], [0xC4, 0x2B, 0x1C])
    } else {
        ([0xFF, 0xFF, 0xFF], [0xFF, 0x63, 0x5A])
    };
    let t = severity.clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        (normal[i] as f32 + (red[i] as f32 - normal[i] as f32) * t).round() as u8
    })
}

pub fn taskbar_is_light() -> bool {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && value != 0
}

/// Ink height of each letter of a stacked two-letter unit ("ms"), as a
/// fraction of the icon size. Fixed, so the number gets everything else.
const STACK_LETTER: f32 = 0.2;
/// Space between the stacked letters, as a fraction of the icon size.
const STACK_GAP: f32 = 0.06;
/// Ink height of a single-letter unit ("s"), as a fraction of the icon size.
const SINGLE_LETTER: f32 = 0.3;
/// Space between the number and the unit, as a fraction of the icon size.
const UNIT_GAP: f32 = 0.05;
/// The number may be squeezed to this fraction of its natural width before it
/// has to get shorter.
const MIN_ASPECT: f32 = 0.7;

/// Lays out `text` from `x` on `baseline`; returns the glyphs and the end x.
fn run(
    font: &FontVec,
    text: &str,
    scale: PxScale,
    mut x: f32,
    baseline: f32,
) -> (Vec<OutlinedGlyph>, f32) {
    let scaled = font.as_scaled(scale);
    let mut prev = None;
    let mut glyphs = Vec::new();
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = prev {
            x += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(scale, point(x, baseline));
        x += scaled.h_advance(id);
        prev = Some(id);
        glyphs.extend(font.outline_glyph(glyph));
    }
    (glyphs, x)
}

/// Ink height of `text` at a 100px scale.
fn height_at_100(font: &FontVec, text: &str) -> f32 {
    let (glyphs, _) = run(font, text, PxScale::from(100.0), 0.0, 100.0);
    bounds(&glyphs).map_or(0.0, |b| b.height())
}

fn bounds(glyphs: &[OutlinedGlyph]) -> Option<Rect> {
    glyphs.iter().map(|g| g.px_bounds()).reduce(|a, b| Rect {
        min: point(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
        max: point(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
    })
}

/// Glyphs laid out at the origin, plus the whole-pixel offset to draw them at
/// (whole pixels keep them exactly as rasterised when measured).
struct Placed {
    glyphs: Vec<OutlinedGlyph>,
    bounds: Rect,
    dx: i32,
    dy: i32,
}

impl Placed {
    fn new(font: &FontVec, text: &str, scale: PxScale) -> Option<Self> {
        let (glyphs, _) = run(font, text, scale, 0.0, 0.0);
        let bounds = bounds(&glyphs)?;
        Some(Self {
            glyphs,
            bounds,
            dx: 0,
            dy: 0,
        })
    }

    /// One letter of a unit, scaled to an ink height of `height` pixels.
    fn letter(font: &FontVec, letter: &str, height: f32) -> Option<Self> {
        let natural = height_at_100(font, letter).max(f32::MIN_POSITIVE);
        Self::new(font, letter, PxScale::from(100.0 * height / natural))
    }

    fn width(&self) -> f32 {
        self.bounds.width()
    }

    /// Moves the ink's left edge to `x` and its bottom edge to `bottom`.
    fn place(&mut self, x: f32, bottom: f32) {
        self.dx = (x - self.bounds.min.x).round() as i32;
        self.dy = (bottom - self.bounds.max.y).round() as i32;
    }

    fn bottom(&self) -> f32 {
        self.bounds.max.y + self.dy as f32
    }

    fn top(&self) -> f32 {
        self.bounds.min.y + self.dy as f32
    }
}

/// Lays out `number`, with an optional `unit` after it, in a `size`×`size`
/// square. The unit is a fixed small size, bottom-aligned with the number: a
/// two-letter unit ("ms") is stacked, first letter over second. The number
/// takes all the remaining room, squeezed horizontally (down to a limit)
/// before it gets shorter, so three digits stay readable.
fn arrange(font: &FontVec, number: &str, unit: Option<&str>, size: u32) -> Option<Vec<Placed>> {
    let side = size as f32;

    // The unit first: its size doesn't depend on the number.
    let letters: Vec<String> = unit.map_or(Vec::new(), |u| u.chars().map(String::from).collect());
    let letter_h = if letters.len() == 2 {
        STACK_LETTER
    } else {
        SINGLE_LETTER
    } * side;
    let mut unit_parts: Vec<Placed> = letters
        .iter()
        .filter_map(|letter| Placed::letter(font, letter, letter_h))
        .collect();
    let column_w = unit_parts.iter().map(Placed::width).fold(0.0, f32::max);
    let unit_w = if unit_parts.is_empty() {
        0.0
    } else {
        column_w + (side * UNIT_GAP).round().max(1.0)
    };

    // Then the number, as large as fits in the rest.
    let measured = Placed::new(font, number, PxScale::from(100.0))?;
    let room = side - unit_w;
    let mut sy = 100.0 * side / measured.bounds.height();
    let mut sx = sy.min(100.0 * room / measured.width());
    sy = sy.min(sx / MIN_ASPECT);
    // That estimate scales a measurement from another size; whole-pixel
    // bounds at the real size can come out a pixel or two wider or taller,
    // which clipped the number or pushed the unit off the edge. Shrink until
    // it really fits.
    let mut digits = loop {
        let digits = Placed::new(font, number, PxScale { x: sx, y: sy })?;
        let (width, height) = (digits.width(), digits.bounds.height());
        if (width <= room && height <= side) || sx < 1.0 {
            break digits;
        }
        if height > side {
            sy *= (side - 0.5) / height;
            sx = sx.min(sy);
        }
        if width > room {
            sx *= (room - 0.5) / width;
            sy = sy.min(sx / MIN_ASPECT);
        }
    };

    // Centre the whole group; the unit sits on the number's bottom edge.
    let left = ((side - digits.width() - unit_w) / 2.0).round();
    let bottom = ((side + digits.bounds.height()) / 2.0).round();
    digits.place(left, bottom);
    let column_x = left + digits.width() + (unit_w - column_w);
    let stack_gap = (side * STACK_GAP).round().max(1.0);
    let mut floor = digits.bottom();
    for part in unit_parts.iter_mut().rev() {
        // Whole pixels, so centring a narrower letter can't round it outward.
        let x = column_x + ((column_w - part.width()) / 2.0).floor();
        part.place(x, floor);
        floor = part.top() - stack_gap;
    }
    unit_parts.insert(0, digits);
    Some(unit_parts)
}

/// Renders [`arrange`]'s layout into a `size`×`size` RGBA image.
pub fn render(
    font: &FontVec,
    number: &str,
    unit: Option<&str>,
    size: u32,
    rgb: [u8; 3],
) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let parts = arrange(font, number, unit, size).unwrap_or_default();

    let mut coverage = vec![0f32; (size * size) as usize];
    for part in &parts {
        for glyph in &part.glyphs {
            let gb = glyph.px_bounds();
            glyph.draw(|x, y, c| {
                let px = gb.min.x as i32 + x as i32 + part.dx;
                let py = gb.min.y as i32 + y as i32 + part.dy;
                if (0..size as i32).contains(&px) && (0..size as i32).contains(&py) {
                    let i = (py as u32 * size + px as u32) as usize;
                    coverage[i] = (coverage[i] + c).min(1.0);
                }
            });
        }
    }

    for (pixel, c) in rgba.chunks_exact_mut(4).zip(coverage) {
        pixel[..3].copy_from_slice(&rgb);
        pixel[3] = (c * 255.0).round() as u8;
    }
    rgba
}
/// Builds an HICON from straight-alpha RGBA pixels. The caller owns the icon.
pub fn create_hicon(rgba: &[u8], size: u32) -> windows::core::Result<HICON> {
    let side = size as i32;
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                biHeight: -side, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
        for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }

        // Monochrome AND mask: rows are WORD-aligned. All zeros; alpha does the work.
        let mask_stride = size.div_ceil(16) as usize * 2;
        let mask_bits = vec![0u8; mask_stride * size as usize];
        let mask = CreateBitmap(side, side, 1, 1, Some(mask_bits.as_ptr() as *const c_void));

        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ping_fits_inside_the_icon() {
        let font = crate::fonts::load("Segoe UI Variable Text", 400)
            .or_else(|| crate::fonts::load("Segoe UI", 400))
            .expect("a Segoe UI font");
        let mut cases: Vec<(String, Option<&str>)> =
            (1..1000).map(|ms| (ms.to_string(), Some("ms"))).collect();
        cases.push(("<1".into(), Some("ms")));
        cases.extend((1..10).map(|s| (s.to_string(), Some("s"))));

        let mut failures = Vec::new();
        // Small-icon sizes at 100% to 300% display scaling.
        for size in [16u32, 20, 24, 28, 32, 36, 40, 48] {
            for (number, unit) in &cases {
                let parts = arrange(&font, number, *unit, size).expect("laid out");
                for part in &parts {
                    let (dx, dy) = (part.dx as f32, part.dy as f32);
                    let (left, right) = (part.bounds.min.x + dx, part.bounds.max.x + dx);
                    let (top, bottom) = (part.bounds.min.y + dy, part.bounds.max.y + dy);
                    if left < 0.0 || top < 0.0 || right > size as f32 || bottom > size as f32 {
                        failures.push(format!("{number}{} at {size}px", unit.unwrap_or("")));
                    }
                }
                // The unit starts to the right of where the number ends.
                let number_right = parts[0].bounds.max.x + parts[0].dx as f32;
                if parts[1..]
                    .iter()
                    .any(|p| p.bounds.min.x + (p.dx as f32) < number_right)
                {
                    failures.push(format!("{number} overlaps its unit at {size}px"));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} clipped: {:?}",
            failures.len(),
            &failures[..failures.len().min(20)]
        );
    }
}
