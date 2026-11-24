//! The window opened from the tray icon: a Stats tab (the default) with a
//! latency and packet-loss chart, and a Settings tab.

use crate::chart;
use crate::config::{self, Config};
use crate::fonts;
use crate::history::History;
use crate::startup;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, GetSysColor,
    GetSysColorBrush, HBRUSH, HDC, HFONT, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    SetBkColor,
};
use windows::Win32::UI::Controls::{
    BST_CHECKED, EM_SETSEL, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR,
    TCIF_TEXT, TCITEMW, TCM_GETCURSEL, TCM_GETITEMRECT, TCM_INSERTITEMW, TCM_SETCURSEL,
    TCN_SELCHANGE, WC_TABCONTROLW,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

const CLASS_NAME: PCWSTR = w!("PingMonitorWindow");
const ID_OK: i32 = 1; // IDOK: Enter in IsDialogMessage
const ID_CANCEL: i32 = 2; // IDCANCEL: Esc in IsDialogMessage
const ID_HOST: i32 = 100;
const ID_INTERVAL: i32 = 101;
const ID_FONT_FAMILY: i32 = 102;
const ID_FONT_WEIGHT: i32 = 103;
const ID_TABS: i32 = 105;
const ID_START_AT_SIGN_IN: i32 = 106;
// Static control styles; windows-rs doesn't export these.
const SS_NOPREFIX: u32 = 0x0080;
const SS_ENDELLIPSIS: u32 = 0x4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Stats = 0,
    Settings = 1,
}

thread_local! {
    static WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
    static FONT: Cell<Option<HFONT>> = const { Cell::new(None) };
    static TABS: Cell<Option<HWND>> = const { Cell::new(None) };
    static SUMMARY: Cell<Option<HWND>> = const { Cell::new(None) };
    static CHART: Cell<Option<HWND>> = const { Cell::new(None) };
    /// Controls on each tab, indexed by `Tab`.
    static PAGES: RefCell<[Vec<HWND>; 2]> = const { RefCell::new([Vec::new(), Vec::new()]) };
}

/// The open window, if any (the message loop routes keys through it).
pub fn hwnd() -> Option<HWND> {
    WINDOW.get()
}

/// The app icon (resource 1, see app.rc) at the size `metric` names, so Windows
/// picks the matching image instead of scaling one. Default icon if missing.
fn app_icon(instance: HINSTANCE, metric: SYSTEM_METRICS_INDEX) -> HICON {
    unsafe {
        let size = GetSystemMetrics(metric);
        LoadImageW(
            Some(instance),
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
        .map(|handle| HICON(handle.0))
        .unwrap_or_default()
    }
}

pub fn register(instance: HINSTANCE) -> windows::core::Result<()> {
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES,
        });
    }
    chart::register(instance)?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        hIcon: app_icon(instance, SM_CXICON),
        hIconSm: app_icon(instance, SM_CXSMICON),
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

fn selected_tab() -> Tab {
    let index = TABS.get().map_or(
        0,
        |tabs| unsafe { SendMessageW(tabs, TCM_GETCURSEL, None, None) }.0,
    );
    if index == Tab::Settings as isize {
        Tab::Settings
    } else {
        Tab::Stats
    }
}

fn select_tab(tab: Tab) {
    unsafe {
        if let Some(tabs) = TABS.get() {
            SendMessageW(tabs, TCM_SETCURSEL, Some(WPARAM(tab as usize)), None);
        }
        PAGES.with_borrow(|pages| {
            for (i, page) in pages.iter().enumerate() {
                let cmd = if i == tab as usize { SW_SHOW } else { SW_HIDE };
                for &ctl in page {
                    let _ = ShowWindow(ctl, cmd);
                }
            }
        });
        let Some(window) = WINDOW.get() else {
            return;
        };
        match tab {
            Tab::Stats => {
                if let Some(tabs) = TABS.get() {
                    let _ = SetFocus(Some(tabs));
                }
            }
            Tab::Settings => {
                if let Ok(host) = GetDlgItem(Some(window), ID_HOST) {
                    let _ = SetFocus(Some(host));
                    SendMessageW(host, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
            }
        }
    }
}

fn format_pct(pct: f32) -> String {
    if pct == pct.round() {
        format!("{pct:.0}%")
    } else {
        format!("{pct:.1}%")
    }
}

fn summary_text(history: &History, host: &str) -> String {
    let samples = history.samples();
    let (Some(first), Some(last), Some(summary)) =
        (samples.front(), samples.back(), history.summary(None))
    else {
        return format!("Waiting for the first ping to {host}…");
    };
    let ms = |v: Option<u32>| v.map_or("–".to_string(), |v| format!("{v} ms"));
    let now = last
        .rtt_ms
        .map_or("lost".to_string(), |v| format!("{v} ms"));
    let minutes = first.at.elapsed().as_secs() / 60;
    let span = match (minutes / 60, minutes % 60) {
        (0, 0) => "under a minute".to_string(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    };
    format!(
        "{host}  ·  now {now}  ·  avg {}  ·  min {}  ·  max {}  ·  {} loss  ·  {} pings in {span}",
        ms(summary.avg_ms),
        ms(summary.min_ms),
        ms(summary.max_ms),
        format_pct(summary.loss_pct),
        summary.count,
    )
}

/// Refreshes the Stats tab, if the window is open.
pub fn update_stats(history: &History, host: &str) {
    if WINDOW.get().is_none() {
        return;
    }
    if let Some(summary) = SUMMARY.get() {
        unsafe {
            let _ = SetWindowTextW(summary, &HSTRING::from(summary_text(history, host)));
        }
    }
    chart::set_data(CHART.get(), history.samples().iter().copied().collect());
}

/// Opens the window on `tab`, or brings it to the front (switching to `tab`)
/// if it's already open.
pub fn show(
    instance: HINSTANCE,
    current: &Config,
    history: &History,
    tab: Tab,
) -> windows::core::Result<()> {
    unsafe {
        if let Some(existing) = WINDOW.get() {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
            select_tab(tab);
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

        // Layout in 96-DPI units. Pages start below the tab strip.
        let (margin, width, height) = (12, 600, 420);
        let (edit_h, label_h, button_w, button_h) = (23, 16, 80, 26);
        let inner = width - 2 * margin;
        let (tabs_y, tabs_h) = (8, 30);
        let page_y = tabs_y + tabs_h + 14;

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let ex_style = WINDOW_EX_STYLE(0);
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
            w!("Ping Monitor"),
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
                WS_CHILD | style,
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

        // Tab strip: header only. Page controls are siblings shown and hidden
        // as the selection changes.
        let tabs = child(
            WC_TABCONTROLW,
            &"".into(),
            WS_VISIBLE | WS_TABSTOP | WS_CLIPSIBLINGS,
            WINDOW_EX_STYLE(0),
            (margin, tabs_y, inner, tabs_h),
            ID_TABS,
        )?;
        TABS.set(Some(tabs));
        for (i, label) in ["Stats", "Settings"].into_iter().enumerate() {
            let mut text: Vec<u16> = label.encode_utf16().chain([0]).collect();
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            SendMessageW(
                tabs,
                TCM_INSERTITEMW,
                Some(WPARAM(i)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
        // Trim the control to its tab headers plus the line under them, so no
        // empty tab body shows.
        let mut header = RECT::default();
        SendMessageW(
            tabs,
            TCM_GETITEMRECT,
            Some(WPARAM(0)),
            Some(LPARAM(&mut header as *mut _ as isize)),
        );
        let _ = SetWindowPos(
            tabs,
            None,
            0,
            0,
            s(inner),
            header.bottom + s(2),
            SWP_NOMOVE | SWP_NOZORDER,
        );

        let mut stats = Vec::new();
        let mut settings = Vec::new();

        // --- Stats ---
        let summary = child(
            w!("STATIC"),
            &summary_text(history, &current.host).into(),
            WINDOW_STYLE(SS_NOPREFIX | SS_ENDELLIPSIS),
            WINDOW_EX_STYLE(0),
            (margin, page_y, inner, label_h + 4),
            -1,
        )?;
        SUMMARY.set(Some(summary));
        stats.push(summary);
        let chart_y = page_y + label_h + 16;
        let chart = child(
            chart::CLASS_NAME,
            &"".into(),
            WINDOW_STYLE(0),
            WINDOW_EX_STYLE(0),
            (margin, chart_y, inner, height - margin - chart_y),
            -1,
        )?;
        CHART.set(Some(chart));
        stats.push(chart);
        chart::set_data(Some(chart), history.samples().iter().copied().collect());

        // --- Settings ---
        let label = |text: &str, rect| {
            child(
                w!("STATIC"),
                &text.into(),
                WINDOW_STYLE(0),
                WINDOW_EX_STYLE(0),
                rect,
                -1,
            )
        };
        let edit = WINDOW_STYLE((ES_AUTOHSCROLL as u32) | WS_TABSTOP.0);
        let list = WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0);
        let weight_w = 150;
        let field_w = 320;

        let mut y = page_y;
        settings.push(label(
            "Host name or IP address:",
            (margin, y, field_w, label_h),
        )?);
        settings.push(child(
            w!("EDIT"),
            &current.host.as_str().into(),
            edit,
            WS_EX_CLIENTEDGE,
            (margin, y + label_h + 2, field_w, edit_h),
            ID_HOST,
        )?);
        y += 54;
        settings.push(label(
            "Ping interval (milliseconds):",
            (margin, y, field_w, label_h),
        )?);
        settings.push(child(
            w!("EDIT"),
            &current.interval_ms.to_string().into(),
            WINDOW_STYLE(edit.0 | ES_NUMBER as u32),
            WS_EX_CLIENTEDGE,
            (margin, y + label_h + 2, 120, edit_h),
            ID_INTERVAL,
        )?);

        // Font: family on its own row, then weight and size side by side. A
        // combo box's height includes its drop-down list.
        y += 54;
        settings.push(label("Icon font:", (margin, y, field_w, label_h))?);
        let family = child(
            w!("COMBOBOX"),
            &"".into(),
            list,
            WINDOW_EX_STYLE(0),
            (margin, y + label_h + 2, field_w, 300),
            ID_FONT_FAMILY,
        )?;
        settings.push(family);
        y += 56;
        settings.push(label("Weight:", (margin, y, weight_w, label_h))?);
        let weight = child(
            w!("COMBOBOX"),
            &"".into(),
            list,
            WINDOW_EX_STYLE(0),
            (margin, y + label_h + 2, weight_w, 300),
            ID_FONT_WEIGHT,
        )?;
        settings.push(weight);

        // Read from Windows, not the config file: Task Manager can change it too.
        y += 64;
        let sign_in = child(
            w!("BUTTON"),
            &"Start Ping Monitor when I sign in".into(),
            WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0),
            WINDOW_EX_STYLE(0),
            (margin, y, field_w, 20),
            ID_START_AT_SIGN_IN,
        )?;
        if startup::is_enabled() {
            SendMessageW(
                sign_in,
                BM_SETCHECK,
                Some(WPARAM(BST_CHECKED.0 as usize)),
                None,
            );
        }
        settings.push(sign_in);

        let buttons_y = height - margin - button_h;
        let cancel_x = width - margin - button_w;
        settings.push(child(
            w!("BUTTON"),
            &"Save".into(),
            WINDOW_STYLE(BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0),
            WINDOW_EX_STYLE(0),
            (cancel_x - 8 - button_w, buttons_y, button_w, button_h),
            ID_OK,
        )?);
        settings.push(child(
            w!("BUTTON"),
            &"Cancel".into(),
            WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0),
            WINDOW_EX_STYLE(0),
            (cancel_x, buttons_y, button_w, button_h),
            ID_CANCEL,
        )?);

        for name in fonts::families() {
            add_item(family, &name);
        }
        let current_family = HSTRING::from(current.font_family.as_str());
        let found = SendMessageW(
            family,
            CB_FINDSTRINGEXACT,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(current_family.as_ptr() as isize)),
        );
        SendMessageW(family, CB_SETCURSEL, Some(WPARAM(found.0 as usize)), None);

        for (_, name) in fonts::WEIGHTS {
            add_item(weight, name);
        }
        let nearest = fonts::WEIGHTS
            .iter()
            .enumerate()
            .min_by_key(|(_, (w, _))| w.abs_diff(current.font_weight))
            .map_or(0, |(i, _)| i);
        SendMessageW(weight, CB_SETCURSEL, Some(WPARAM(nearest)), None);

        PAGES.set([stats, settings]);
        select_tab(tab);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        select_tab(tab); // again, now the window can take focus
        Ok(())
    }
}

fn add_item(combo: HWND, text: &str) {
    let text = HSTRING::from(text);
    unsafe {
        SendMessageW(
            combo,
            CB_ADDSTRING,
            Some(WPARAM(0)),
            Some(LPARAM(text.as_ptr() as isize)),
        );
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

    let font_family = control_text(hwnd, ID_FONT_FAMILY);
    if font_family.is_empty() {
        return complain(hwnd, "Choose a font for the icon.", ID_FONT_FAMILY);
    }
    let weight_index = unsafe {
        GetDlgItem(Some(hwnd), ID_FONT_WEIGHT)
            .map(|combo| SendMessageW(combo, CB_GETCURSEL, None, None).0)
            .unwrap_or(-1)
    };
    let font_weight = usize::try_from(weight_index)
        .ok()
        .and_then(|i| fonts::WEIGHTS.get(i))
        .map_or(400, |(weight, _)| *weight);

    let new = Config {
        host,
        interval_ms,
        font_family,
        font_weight,
    };
    let start_at_sign_in = unsafe {
        GetDlgItem(Some(hwnd), ID_START_AT_SIGN_IN).is_ok_and(|check| {
            SendMessageW(check, BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize
        })
    };

    let result = crate::apply_config(new).and_then(|()| {
        if start_at_sign_in != startup::is_enabled() {
            startup::set_enabled(start_at_sign_in)?;
        }
        Ok(())
    });
    match result {
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
            // Enter only saves on the Settings tab.
            ID_OK => {
                if selected_tab() == Tab::Settings {
                    save(hwnd);
                }
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
        WM_NOTIFY => {
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            if header.code == TCN_SELCHANGE {
                select_tab(selected_tab());
            }
        }
        // Labels sit on the white window background, not the dialog grey.
        WM_CTLCOLORSTATIC => unsafe {
            SetBkColor(
                HDC(wparam.0 as *mut c_void),
                COLORREF(GetSysColor(COLOR_WINDOW)),
            );
            return LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize);
        },
        WM_DESTROY => {
            WINDOW.set(None);
            TABS.set(None);
            SUMMARY.set(None);
            CHART.set(None);
            PAGES.set([Vec::new(), Vec::new()]);
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
