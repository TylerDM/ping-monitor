#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod icon;
mod ping;
mod settings;

use ab_glyph::FontVec;
use config::Config;
use ping::{PingError, Report};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("PingMonitorTray");
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_PING: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;
/// NIN_SELECT | NINF_KEY; windows-rs doesn't export it.
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;
const MENU_SETTINGS: usize = 1;
const MENU_QUIT: usize = 2;
/// Samples kept for the tooltip's average / loss figures.
const HISTORY_LEN: usize = 60;
/// Latency colour: normal text up to this many milliseconds...
const NORMAL_UP_TO_MS: u32 = 50;
/// ...blending to fully red at this many.
const RED_FROM_MS: u32 = 200;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug)]
enum Status {
    Pending,
    Reply(u32),
    Timeout,
    Error(String),
}

struct App {
    hwnd: HWND,
    instance: HINSTANCE,
    config: Config,
    config_tx: Sender<Config>,
    reports: Receiver<Report>,
    status: Status,
    history: VecDeque<Option<u32>>,
    font: FontVec,
    icon: Option<HICON>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` against the app state. Returns `None` if the state isn't set up yet
/// or is already borrowed further up the stack (re-entrant window messages).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut app = cell.try_borrow_mut().ok()?;
        app.as_mut().map(f)
    })
}

impl App {
    fn notify_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        }
    }

    fn add_tray(&mut self) {
        self.rebuild_icon();
        let mut data = self.notify_data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_APP_TRAY;
        data.hIcon = self.icon.unwrap_or_default();
        data.szTip = self.tooltip();
        unsafe {
            // Fails if Explorer isn't up yet; TaskbarCreated will bring us back.
            if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
            }
        }
    }

    fn remove_tray(&mut self) {
        let data = self.notify_data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }

    fn rebuild_icon(&mut self) {
        // (number, unit, severity from 0 = fine to 1 = red)
        let (number, unit, severity) = match &self.status {
            Status::Pending => ("…".to_string(), None, 0.0),
            Status::Reply(0) => ("<1".to_string(), Some("ms"), 0.0),
            Status::Reply(ms) if *ms < 1000 => {
                let severity =
                    (*ms as f32 - NORMAL_UP_TO_MS as f32) / (RED_FROM_MS - NORMAL_UP_TO_MS) as f32;
                (ms.to_string(), Some("ms"), severity)
            }
            Status::Reply(ms) => ((ms / 1000).to_string(), Some("s"), 1.0),
            Status::Timeout => ("×".to_string(), None, 1.0),
            Status::Error(_) => ("!".to_string(), None, 1.0),
        };
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
        let rgba = icon::render(
            &self.font,
            &number,
            unit,
            size,
            icon::color(severity, icon::taskbar_is_light()),
        );
        if let Ok(new) = icon::create_hicon(&rgba, size) {
            if let Some(old) = self.icon.replace(new) {
                unsafe {
                    let _ = DestroyIcon(old);
                }
            }
        }
    }

    fn tooltip(&self) -> [u16; 128] {
        let last = match &self.status {
            Status::Pending => "Waiting for first reply…".to_string(),
            Status::Reply(0) => "<1 ms".to_string(),
            Status::Reply(ms) => format!("{ms} ms"),
            Status::Timeout => "Timed out".to_string(),
            Status::Error(e) => e.clone(),
        };
        let mut text = format!("Ping {}: {last}", self.config.host);
        let replies: Vec<u32> = self.history.iter().flatten().copied().collect();
        if !self.history.is_empty() {
            let loss = 100 * (self.history.len() - replies.len()) / self.history.len();
            let avg = if replies.is_empty() {
                "–".to_string()
            } else {
                (replies.iter().map(|&ms| ms as u64).sum::<u64>() / replies.len() as u64)
                    .to_string()
            };
            text += &format!("\nAvg {avg} ms, {loss}% loss (last {})", self.history.len());
        }

        let mut tip = [0u16; 128];
        for (dst, src) in tip.iter_mut().take(127).zip(text.encode_utf16()) {
            *dst = src;
        }
        tip
    }

    /// Pushes the current icon and tooltip to the tray.
    fn refresh(&mut self) {
        self.rebuild_icon();
        let mut data = self.notify_data();
        data.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.hIcon = self.icon.unwrap_or_default();
        data.szTip = self.tooltip();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn drain_reports(&mut self) {
        let mut changed = false;
        while let Ok(report) = self.reports.try_recv() {
            // Results that were in flight when the host changed.
            if report.host != self.config.host {
                continue;
            }
            self.status = match report.result {
                Ok(ms) => Status::Reply(ms),
                Err(PingError::Timeout) => Status::Timeout,
                Err(PingError::Failed(e)) => Status::Error(e),
            };
            if self.history.len() == HISTORY_LEN {
                self.history.pop_front();
            }
            self.history.push_back(match self.status {
                Status::Reply(ms) => Some(ms),
                _ => None,
            });
            changed = true;
        }
        if changed {
            self.refresh();
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(icon) = self.icon.take() {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

/// Saves new settings and applies them. Called by the settings window.
pub fn apply_config(new: Config) -> Result<(), String> {
    config::save(&new).map_err(|e| e.to_string())?;
    with_app(|app| {
        if app.config.host != new.host {
            app.status = Status::Pending;
            app.history.clear();
        }
        app.config = new.clone();
        let _ = app.config_tx.send(new);
        app.refresh();
    })
    .ok_or_else(|| "The app is busy; try again.".to_string())
}

fn open_settings() {
    let Some((instance, config)) = with_app(|app| (app.instance, app.config.clone())) else {
        return;
    };
    if let Err(e) = settings::show(instance, &config) {
        error_box(&format!("Couldn't open settings: {}", e.message()));
    }
}

fn show_menu(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Settings…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));
        let _ = SetMenuDefaultItem(menu, MENU_SETTINGS as u32, 0);

        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            x,
            y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);

        match cmd.0 as usize {
            MENU_SETTINGS => open_settings(),
            MENU_QUIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: the event is in LOWORD(lparam), the anchor point in wparam.
            match (lparam.0 & 0xFFFF) as u32 {
                NIN_SELECT | NIN_KEYSELECT => open_settings(),
                WM_CONTEXTMENU => {
                    let x = (wparam.0 & 0xFFFF) as i16 as i32;
                    let y = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
                    show_menu(hwnd, x, y);
                }
                _ => {}
            }
            return LRESULT(0);
        }
        WM_APP_PING => {
            with_app(App::drain_reports);
            return LRESULT(0);
        }
        // Light/dark theme switch: recolour the icon.
        WM_SETTINGCHANGE => {
            with_app(App::refresh);
        }
        WM_DESTROY => {
            with_app(App::remove_tray);
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted; our icon is gone.
            with_app(App::add_tray);
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn error_box(message: &str) {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("Ping Monitor"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let _instance_lock = CreateMutexW(None, false, w!(r"Local\PingMonitor.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            MessageBoxW(
                None,
                w!("Ping Monitor is already running. Look for it in the notification area."),
                w!("Ping Monitor"),
                MB_OK | MB_ICONINFORMATION,
            );
            return Ok(());
        }

        let font = icon::load_font().ok_or("No usable font found in the Windows Fonts folder")?;
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();

        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        settings::register(instance)?;
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );

        // A hidden top-level window (not message-only) so it receives
        // TaskbarCreated and WM_SETTINGCHANGE broadcasts.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Ping Monitor"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?;

        let config = config::load();
        let (config_tx, reports) = ping::spawn_worker(config.clone(), hwnd, WM_APP_PING)?;
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                instance,
                config,
                config_tx,
                reports,
                status: Status::Pending,
                history: VecDeque::with_capacity(HISTORY_LEN),
                font,
                icon: None,
            })
        });
        with_app(App::add_tray);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if let Some(dialog) = settings::hwnd() {
                if IsDialogMessageW(dialog, &msg).as_bool() {
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        APP.with(|cell| cell.borrow_mut().take());
        Ok(())
    }
}

fn main() {
    if let Err(e) = run() {
        error_box(&format!("Ping Monitor failed to start:\n{e}"));
        std::process::exit(1);
    }
}
