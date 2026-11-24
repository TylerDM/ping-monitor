//! The settings window shown when the tray icon is clicked.

use crate::config::{self, Config};
use std::cell::Cell;
use std::ffi::c_void;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, HBRUSH, HFONT,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("PingMonitorSettings");
const ID_OK: i32 = 1; // IDOK: Enter in IsDialogMessage
const ID_CANCEL: i32 = 2; // IDCANCEL: Esc in IsDialogMessage
const ID_HOST: i32 = 100;
const ID_INTERVAL: i32 = 101;
/// From Win32_UI_Controls; defined here rather than pull in that whole feature.
const EM_SETSEL: u32 = 0x00B1;

thread_local! {
    static WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
    static FONT: Cell<Option<HFONT>> = const { Cell::new(None) };
}

/// The open settings window, if any (the message loop routes keys through it).
pub fn hwnd() -> Option<HWND> {
    WINDOW.get()
}

pub fn register(instance: HINSTANCE) -> windows::core::Result<()> {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut c_void),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Opens the settings window, or brings it to the front if it's already open.
pub fn show(instance: HINSTANCE, current: &Config) -> windows::core::Result<()> {
    unsafe {
        if let Some(existing) = WINDOW.get() {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
            return Ok(());
        }

        // Centre on the monitor the cursor is on (i.e. the one with the clicked tray).
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut dpi_y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
        let s = |v: i32| v * dpi as i32 / 96;

        // Layout in 96-DPI units.
        let (margin, width, edit_h, label_h, button_w, button_h) = (12, 320, 23, 16, 80, 26);
        let inner = width - 2 * margin;
        let buttons_y = 120;
        let height = buttons_y + button_h + margin;

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_DLGMODALFRAME;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: s(width),
            bottom: s(height),
        };
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, dpi);
        let (frame_w, frame_h) = (frame.right - frame.left, frame.bottom - frame.top);
        let work = info.rcWork;
        let x = work.left + (work.right - work.left - frame_w) / 2;
        let y = work.top + (work.bottom - work.top - frame_h) / 2;

        let hwnd = CreateWindowExW(
            ex_style,
            CLASS_NAME,
            w!("Ping Monitor Settings"),
            style,
            x,
            y,
            frame_w,
            frame_h,
            None,
            None,
            Some(instance),
            None,
        )?;
        WINDOW.set(Some(hwnd));

        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let font = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut c_void),
            0,
            dpi,
        )
        .ok()
        .map(|_| CreateFontIndirectW(&metrics.lfMessageFont));
        FONT.set(font);

        let child = |class: PCWSTR,
                     text: &HSTRING,
                     style: WINDOW_STYLE,
                     ex: WINDOW_EX_STYLE,
                     (x, y, w, h): (i32, i32, i32, i32),
                     id: i32|
         -> windows::core::Result<HWND> {
            let ctl = CreateWindowExW(
                ex,
                class,
                text,
                WS_CHILD | WS_VISIBLE | style,
                s(x),
                s(y),
                s(w),
                s(h),
                Some(hwnd),
                Some(HMENU(id as isize as *mut c_void)),
                Some(instance),
                None,
            )?;
            if let Some(font) = font {
                SendMessageW(
                    ctl,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            Ok(ctl)
        };

        let edit = WINDOW_STYLE((ES_AUTOHSCROLL as u32) | WS_TABSTOP.0);
        child(
            w!("STATIC"),
            &"Host name or IP address:".into(),
            WINDOW_STYLE(0),
            WINDOW_EX_STYLE(0),
            (margin, margin, inner, label_h),
            -1,
        )?;
        let host = child(
            w!("EDIT"),
            &current.host.as_str().into(),
            edit,
            WS_EX_CLIENTEDGE,
            (margin, margin + label_h + 2, inner, edit_h),
            ID_HOST,
        )?;
        child(
            w!("STATIC"),
            &"Ping interval (milliseconds):".into(),
            WINDOW_STYLE(0),
            WINDOW_EX_STYLE(0),
            (margin, 66, inner, label_h),
            -1,
        )?;
        child(
            w!("EDIT"),
            &current.interval_ms.to_string().into(),
            WINDOW_STYLE(edit.0 | ES_NUMBER as u32),
            WS_EX_CLIENTEDGE,
            (margin, 66 + label_h + 2, 120, edit_h),
            ID_INTERVAL,
        )?;
        let cancel_x = width - margin - button_w;
        child(
            w!("BUTTON"),
            &"Save".into(),
            WINDOW_STYLE(BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0),
            WINDOW_EX_STYLE(0),
            (cancel_x - 8 - button_w, buttons_y, button_w, button_h),
            ID_OK,
        )?;
        child(
            w!("BUTTON"),
            &"Cancel".into(),
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            WINDOW_EX_STYLE(0),
            (cancel_x, buttons_y, button_w, button_h),
            ID_CANCEL,
        )?;

        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(host));
        SendMessageW(host, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        Ok(())
    }
}

fn control_text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let Ok(ctl) = GetDlgItem(Some(hwnd), id) else {
            return String::new();
        };
        let mut buf = vec![0u16; GetWindowTextLengthW(ctl) as usize + 1];
        let len = GetWindowTextW(ctl, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

fn complain(hwnd: HWND, message: &str, focus_id: i32) {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(message),
            w!("Ping Monitor"),
            MB_OK | MB_ICONWARNING,
        );
        if let Ok(ctl) = GetDlgItem(Some(hwnd), focus_id) {
            let _ = SetFocus(Some(ctl));
            SendMessageW(ctl, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }
}

fn save(hwnd: HWND) {
    let host = control_text(hwnd, ID_HOST).trim().to_string();
    if host.is_empty() {
        return complain(hwnd, "Enter a host name or IP address.", ID_HOST);
    }
    let interval_ms = match control_text(hwnd, ID_INTERVAL).trim().parse::<u32>() {
        Ok(ms) if (config::MIN_INTERVAL_MS..=config::MAX_INTERVAL_MS).contains(&ms) => ms,
        _ => {
            let message = format!(
                "The ping interval must be a whole number of milliseconds between {} and {}.",
                config::MIN_INTERVAL_MS,
                config::MAX_INTERVAL_MS
            );
            return complain(hwnd, &message, ID_INTERVAL);
        }
    };

    match crate::apply_config(Config { host, interval_ms }) {
        Ok(()) => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        Err(e) => unsafe {
            MessageBoxW(
                Some(hwnd),
                &HSTRING::from(format!("Couldn't save settings:\n{e}")),
                w!("Ping Monitor"),
                MB_OK | MB_ICONERROR,
            );
        },
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => match (wparam.0 & 0xFFFF) as i32 {
            ID_OK => {
                save(hwnd);
                return LRESULT(0);
            }
            ID_CANCEL => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            _ => {}
        },
        WM_DESTROY => {
            WINDOW.set(None);
            if let Some(font) = FONT.take() {
                unsafe {
                    let _ = DeleteObject(font.into());
                }
            }
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
