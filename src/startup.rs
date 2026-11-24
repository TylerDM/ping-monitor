//! Starting at sign-in.
//!
//! The installer registers the app under the per-user `Run` key. Switching it
//! off doesn't delete that entry: like Task Manager's Startup apps page, it
//! marks it disabled under `StartupApproved\Run`. So the setting and Task
//! Manager always agree, an upgrade (which rewrites the `Run` entry) doesn't
//! switch startup back on, and uninstalling still removes the entry.

use std::ffi::c_void;
use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegGetValueW,
    RegSetKeyValueW,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::core::{PCWSTR, w};

const RUN: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const APPROVED: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
/// Must match the `Run` value the installer writes (installer/PingMonitor.wxs).
const NAME: PCWSTR = w!("PingMonitor");

fn has_run_entry() -> bool {
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            NAME,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS
}

/// The `StartupApproved` flag's first byte, if there is one: even means
/// enabled (2), odd means disabled (3).
fn approval_flag() -> Option<u8> {
    let mut data = [0u8; 12];
    let mut size = data.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            APPROVED,
            NAME,
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS && size > 0).then_some(data[0])
}

/// Whether Ping Monitor will start at the next sign-in.
pub fn is_enabled() -> bool {
    has_run_entry() && approval_flag().is_none_or(|flag| flag & 1 == 0)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let fail = |what: &str, status: windows::Win32::Foundation::WIN32_ERROR| {
        format!(
            "{what}: {}",
            windows::core::Error::from(status.to_hresult()).message()
        )
    };
    unsafe {
        // A copy that wasn't installed (e.g. a dev build) has no Run entry
        // yet; point one at this exe.
        if enabled && !has_run_entry() {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let command: Vec<u16> = format!("\"{}\"", exe.display())
                .encode_utf16()
                .chain([0])
                .collect();
            let status = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN,
                NAME,
                REG_SZ.0,
                Some(command.as_ptr() as *const c_void),
                (command.len() * 2) as u32,
            );
            if status != ERROR_SUCCESS {
                return Err(fail("Couldn't add the startup entry", status));
            }
        }

        // Same 12-byte layout Task Manager writes: a flag, then (when
        // disabled) the time it was switched off.
        let mut data = [0u8; 12];
        if enabled {
            data[0] = 2;
        } else {
            data[0] = 3;
            let now: FILETIME = GetSystemTimeAsFileTime();
            data[4..8].copy_from_slice(&now.dwLowDateTime.to_le_bytes());
            data[8..12].copy_from_slice(&now.dwHighDateTime.to_le_bytes());
        }
        let status = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            APPROVED,
            NAME,
            REG_BINARY.0,
            Some(data.as_ptr() as *const c_void),
            data.len() as u32,
        );
        if status != ERROR_SUCCESS {
            return Err(fail("Couldn't change the startup setting", status));
        }
    }
    Ok(())
}
