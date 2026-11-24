use ab_glyph::{Font, FontVec, OutlinedGlyph, PxScale, Rect, ScaleFont, VariableFont, point};
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

/// The Windows UI font: Segoe UI Variable (Windows 11) at Regular weight and the
/// "Text" optical size the taskbar clock uses, else Segoe UI (Windows 10).
pub fn load_font() -> Option<FontVec> {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let fonts = std::path::Path::new(&windir).join("Fonts");
    let load = |name: &str| {
        std::fs::read(fonts.join(name))
            .ok()
            .and_then(|data| FontVec::try_from_vec(data).ok())
    };

    if let Some(mut font) = load("SegUIVar.ttf") {
        font.set_variation(b"wght", 400.0);
        font.set_variation(b"opsz", 10.5);
        return Some(font);
    }
    ["segoeui.ttf", "arial.ttf"].into_iter().find_map(load)
}

/// Height of the unit ("ms") relative to the number.
const UNIT_SCALE: f32 = 0.5;
/// Space between the number and the unit, in ems of the number.
const UNIT_GAP: f32 = 0.06;

/// Lays out `number` followed by an optional smaller `unit` on the same baseline.
fn layout(font: &FontVec, number: &str, unit: Option<&str>, scale: PxScale) -> Vec<OutlinedGlyph> {
    let baseline = font.as_scaled(scale).ascent();
    let mut x = 0.0;
    let mut glyphs = Vec::new();
    let mut run = |text: &str, scale: PxScale, x: &mut f32| {
        let scaled = font.as_scaled(scale);
        let mut prev = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(prev) = prev {
                *x += scaled.kern(prev, id);
            }
            let glyph = id.with_scale_and_position(scale, point(*x, baseline));
            *x += scaled.h_advance(id);
            prev = Some(id);
            glyphs.extend(font.outline_glyph(glyph));
        }
    };

    run(number, scale, &mut x);
    if let Some(unit) = unit {
        x += scale.x * UNIT_GAP;
        let small = PxScale {
            x: scale.x * UNIT_SCALE,
            y: scale.y * UNIT_SCALE,
        };
        run(unit, small, &mut x);
    }
    glyphs
}

fn bounds(glyphs: &[OutlinedGlyph]) -> Option<Rect> {
    glyphs.iter().map(|g| g.px_bounds()).reduce(|a, b| Rect {
        min: point(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
        max: point(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
    })
}

/// Renders `number` (plus a small `unit` after it) as large as fits in a
/// `size`×`size` RGBA image, centred. Wide strings are squeezed horizontally
/// (down to a limit) before shrinking, so three digits stay readable in a
/// 16px tray icon.
pub fn render(
    font: &FontVec,
    number: &str,
    unit: Option<&str>,
    size: u32,
    rgb: [u8; 3],
) -> Vec<u8> {
    const MIN_ASPECT: f32 = 0.7;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let side = size as f32;

    let Some(measured) = bounds(&layout(font, number, unit, PxScale::from(100.0))) else {
        return rgba;
    };
    let mut sy = 100.0 * side / measured.height();
    let mut sx = 100.0 * side / measured.width();
    if sx > sy {
        sx = sy;
    } else if sx < sy * MIN_ASPECT {
        sy = sx / MIN_ASPECT;
    }

    let glyphs = layout(font, number, unit, PxScale { x: sx, y: sy });
    let Some(b) = bounds(&glyphs) else {
        return rgba;
    };
    // Whole-pixel offsets keep the rasterised glyphs crisp.
    let dx = ((side - b.width()) / 2.0 - b.min.x).round() as i32;
    let dy = ((side - b.height()) / 2.0 - b.min.y).round() as i32;

    let mut coverage = vec![0f32; (size * size) as usize];
    for glyph in &glyphs {
        let gb = glyph.px_bounds();
        glyph.draw(|x, y, c| {
            let px = gb.min.x as i32 + x as i32 + dx;
            let py = gb.min.y as i32 + y as i32 + dy;
            if (0..size as i32).contains(&px) && (0..size as i32).contains(&py) {
                let i = (py as u32 * size + px as u32) as usize;
                coverage[i] = (coverage[i] + c).min(1.0);
            }
        });
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
