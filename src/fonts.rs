//! Finds installed fonts by family name through DirectWrite and loads them
//! for ab_glyph to rasterise.

use ab_glyph::{FontVec, VariableFont};
use std::ffi::c_void;
use std::path::PathBuf;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, HSTRING, Interface, w};

/// Weights offered in the settings window, with their usual names.
pub const WEIGHTS: [(u16, &str); 10] = [
    (100, "Thin"),
    (200, "Extra Light"),
    (300, "Light"),
    (350, "Semilight"),
    (400, "Regular"),
    (500, "Medium"),
    (600, "Semibold"),
    (700, "Bold"),
    (800, "Extra Bold"),
    (900, "Black"),
];

fn system_fonts() -> windows::core::Result<IDWriteFontCollection> {
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let mut collection = None;
        factory.GetSystemFontCollection(&mut collection, false)?;
        collection.ok_or_else(|| E_FAIL.into())
    }
}

fn family_name(family: &IDWriteFontFamily) -> Option<String> {
    unsafe {
        let names = family.GetFamilyNames().ok()?;
        let (mut index, mut exists) = (0, BOOL(0));
        let _ = names.FindLocaleName(w!("en-us"), &mut index, &mut exists);
        if !exists.as_bool() {
            index = 0;
        }
        let len = names.GetStringLength(index).ok()?;
        let mut buf = vec![0u16; len as usize + 1];
        names.GetString(index, &mut buf).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Installed font family names, sorted.
pub fn families() -> Vec<String> {
    let Ok(fonts) = system_fonts() else {
        return Vec::new();
    };
    let mut names: Vec<String> = (0..unsafe { fonts.GetFontFamilyCount() })
        .filter_map(|i| unsafe { fonts.GetFontFamily(i) }.ok())
        .filter_map(|family| family_name(&family))
        // Vertical-text variants of CJK fonts.
        .filter(|name| !name.starts_with('@'))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

fn file_path(face: &IDWriteFontFace) -> Option<PathBuf> {
    unsafe {
        let mut count = 0;
        face.GetFiles(&mut count, None).ok()?;
        let mut files: Vec<Option<IDWriteFontFile>> = vec![None; count as usize];
        face.GetFiles(&mut count, Some(files.as_mut_ptr())).ok()?;
        let file = files.into_iter().next()??;

        let mut key: *mut c_void = std::ptr::null_mut();
        let mut key_size = 0;
        file.GetReferenceKey(&mut key, &mut key_size).ok()?;
        let loader: IDWriteLocalFontFileLoader = file.GetLoader().ok()?.cast().ok()?;
        let len = loader.GetFilePathLengthFromKey(key, key_size).ok()?;
        let mut buf = vec![0u16; len as usize + 1];
        loader.GetFilePathFromKey(key, key_size, &mut buf).ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}

/// Loads the installed font closest to `family` at `weight`, or `None` if no
/// such family is installed.
pub fn load(family: &str, weight: u16) -> Option<FontVec> {
    unsafe {
        let fonts = system_fonts().ok()?;
        let (mut index, mut exists) = (0, BOOL(0));
        fonts
            .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
            .ok()?;
        if !exists.as_bool() {
            return None;
        }
        let face = fonts
            .GetFontFamily(index)
            .ok()?
            .GetFirstMatchingFont(
                DWRITE_FONT_WEIGHT(weight.into()),
                DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
            .ok()?
            .CreateFontFace()
            .ok()?;
        let data = std::fs::read(file_path(&face)?).ok()?;
        let mut font = FontVec::try_from_vec_and_index(data, face.GetIndex()).ok()?;

        // Variable fonts: start from the instance DirectWrite matched (which
        // carries things like Segoe UI Variable Text's optical size), then set
        // the exact weight asked for. Static fonts ignore both.
        if let Ok(face5) = face.cast::<IDWriteFontFace5>() {
            let mut values =
                vec![DWRITE_FONT_AXIS_VALUE::default(); face5.GetFontAxisValueCount() as usize];
            if face5.GetFontAxisValues(&mut values).is_ok() {
                for value in values {
                    font.set_variation(&value.axisTag.0.to_le_bytes(), value.value);
                }
            }
        }
        font.set_variation(b"wght", weight.into());
        Some(font)
    }
}
