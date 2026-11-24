#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chart;
mod config;
mod fonts;
mod history;
mod http;
mod icon;
mod ping;
mod startup;
mod update;
mod window;

use ab_glyph::FontVec;
use config::Config;
use history::History;
use ping::{PingError, Report};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("PingMonitorTray");
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_PING: u32 = WM_APP + 2;
const WM_APP_UPDATE: u32 = WM_APP + 3;
const TRAY_ID: u32 = 1;
/// NIN_SELECT | NINF_KEY; windows-rs doesn't export it.
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;
/// The user clicked our notification (WM_USER + 5).
const NIN_BALLOONUSERCLICK: u32 = NIN_SELECT + 5;
const MENU_OPEN: usize = 1;
const MENU_SETTINGS: usize = 2;
const MENU_QUIT: usize = 3;
const MENU_CHECK_UPDATES: usize = 4;
const MENU_INSTALL_UPDATE: usize = 5;
/// Pings the tooltip's average / loss figures cover.
const TOOLTIP_SAMPLES: usize = 60;
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
    history: History,
    font: FontVec,
    icon: Option<HICON>,
    update_tx: Sender<update::Event>,
    update_rx: Receiver<update::Event>,
    /// A newer release, once a check has found one.
    update: Option<update::Release>,
    /// The version we last showed an "update available" notification for.
    notified_version: Option<String>,
    /// An update is downloading; don't start another.
    installing: bool,
}

/// Copies `text` into a fixed, null-terminated UTF-16 field, truncating it.
fn fill(field: &mut [u16], text: &str) {
    field.fill(0);
    let room = field.len() - 1;
    for (dst, src) in field[..room].iter_mut().zip(text.encode_utf16()) {
        *dst = src;
    }
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

    /// Shows a notification from the tray icon; clicking it sends NIN_BALLOONUSERCLICK.
    fn notify(&self, title: &str, text: &str) {
        let mut data = self.notify_data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        fill(&mut data.szInfoTitle, title);
        fill(&mut data.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
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
        if let Some(summary) = self.history.summary(Some(TOOLTIP_SAMPLES)) {
            let avg = summary.avg_ms.map_or("–".to_string(), |ms| ms.to_string());
            text += &format!(
                "\nAvg {avg} ms, {:.0}% loss (last {})",
                summary.loss_pct, summary.count
            );
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
            self.history.push(match self.status {
                Status::Reply(ms) => Some(ms),
                _ => None,
            });
            changed = true;
        }
        if changed {
            self.refresh();
            window::update_stats(&self.history, &self.config.host);
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

/// Saves new settings and applies them. Called by the Settings tab.
pub fn apply_config(new: Config) -> Result<(), String> {
    let font_changed = with_app(|app| {
        (app.config.font_family != new.font_family) || (app.config.font_weight != new.font_weight)
    })
    .unwrap_or(true);
    let font = if font_changed {
        let font = fonts::load(&new.font_family, new.font_weight)
            .ok_or_else(|| format!("The font \"{}\" isn't installed.", new.font_family))?;
        Some(font)
    } else {
        None
    };

    config::save(&new).map_err(|e| e.to_string())?;
    with_app(|app| {
        if app.config.host != new.host {
            app.status = Status::Pending;
            app.history.clear();
        }
        if let Some(font) = font {
            app.font = font;
        }
        app.config = new.clone();
        let _ = app.config_tx.send(new);
        app.refresh();
        window::update_stats(&app.history, &app.config.host);
    })
    .ok_or_else(|| "The app is busy; try again.".to_string())
}

fn open_window(tab: window::Tab) {
    let Some((instance, config, history)) =
        with_app(|app| (app.instance, app.config.clone(), app.history.clone()))
    else {
        return;
    };
    if let Err(e) = window::show(instance, &config, &history, tab) {
        error_box(&format!("Couldn't open Ping Monitor: {}", e.message()));
    }
}

/// What to do about update events once the app state is no longer borrowed
/// (these show message boxes, which pump messages).
enum UpdateAction {
    UpToDate,
    Failed(String),
    Offer(update::Release),
    Install(PathBuf),
}

fn check_for_updates() {
    with_app(|app| update::spawn_manual_check(app.update_tx.clone(), app.hwnd, WM_APP_UPDATE));
}

/// Downloads the available update; the install starts when it arrives.
fn start_update() {
    with_app(|app| {
        let Some(release) = app.update.clone() else {
            return;
        };
        if !app.installing {
            app.installing = true;
            update::spawn_download(app.update_tx.clone(), app.hwnd, WM_APP_UPDATE, release);
        }
    });
}

fn handle_update_events() {
    let actions = with_app(|app| {
        let mut actions = Vec::new();
        while let Ok(event) = app.update_rx.try_recv() {
            match event {
                update::Event::Checked { manual, result } => match result {
                    Ok(Some(release)) => {
                        app.update = Some(release.clone());
                        if manual {
                            actions.push(UpdateAction::Offer(release));
                        } else if app.notified_version.as_ref() != Some(&release.version) {
                            app.notified_version = Some(release.version.clone());
                            app.notify(
                                "Ping Monitor update available",
                                &format!(
                                    "Version {} is ready. Click here to install it.",
                                    release.version
                                ),
                            );
                        }
                    }
                    Ok(None) if manual => actions.push(UpdateAction::UpToDate),
                    Err(e) if manual => actions.push(UpdateAction::Failed(e)),
                    // Background checks fail quietly; the next one may work.
                    _ => {}
                },
                update::Event::Downloaded(Ok(msi)) => actions.push(UpdateAction::Install(msi)),
                update::Event::Downloaded(Err(e)) => {
                    app.installing = false;
                    actions.push(UpdateAction::Failed(e));
                }
            }
        }
        actions
    })
    .unwrap_or_default();

    for action in actions {
        match action {
            UpdateAction::UpToDate => info_box(&format!(
                "You have the latest version of Ping Monitor ({}).",
                update::CURRENT_VERSION
            )),
            UpdateAction::Failed(e) => error_box(&format!("Couldn't update Ping Monitor:\n{e}")),
            UpdateAction::Offer(release) => {
                let answer = unsafe {
                    MessageBoxW(
                        None,
                        &HSTRING::from(format!(
                            "Ping Monitor {} is available (you have {}).\n\nInstall it now?",
                            release.version,
                            update::CURRENT_VERSION
                        )),
                        w!("Ping Monitor"),
                        MB_YESNO | MB_ICONQUESTION,
                    )
                };
                if answer == IDYES {
                    start_update();
                }
            }
            UpdateAction::Install(msi) => match update::install(&msi) {
                // The installer replaces this exe, so get out of its way; it
                // starts the new version when it's done.
                Ok(()) => {
                    if let Some(hwnd) = with_app(|app| app.hwnd) {
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                }
                Err(e) => {
                    with_app(|app| app.installing = false);
                    error_box(&format!("Couldn't start the installer:\n{e}"));
                }
            },
        }
    }
}

fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let available = with_app(|app| app.update.as_ref().map(|r| r.version.clone())).flatten();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let title = HSTRING::from(format!("Ping Monitor {}", update::CURRENT_VERSION));
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &title);
        if let Some(version) = &available {
            let label = HSTRING::from(format!("Install update ({version})"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_INSTALL_UPDATE, &label);
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Settings…"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_CHECK_UPDATES, w!("Check for updates"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));
        let _ = SetMenuDefaultItem(menu, MENU_OPEN as u32, 0);

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
            MENU_OPEN => open_window(window::Tab::Stats),
            MENU_SETTINGS => open_window(window::Tab::Settings),
            MENU_CHECK_UPDATES => check_for_updates(),
            MENU_INSTALL_UPDATE => start_update(),
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
                NIN_SELECT | NIN_KEYSELECT => open_window(window::Tab::Stats),
                NIN_BALLOONUSERCLICK => start_update(),
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
        WM_APP_UPDATE => {
            handle_update_events();
            return LRESULT(0);
        }
        // Sign-out, or an installer asking us to close (Restart Manager):
        // exit cleanly so the tray icon goes and the exe is free to replace.
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
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

fn info_box(message: &str) {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("Ping Monitor"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
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

        let config = config::load();
        // Fall back to Segoe UI if the configured font is gone (or this is Windows 10,
        // which has no Segoe UI Variable).
        let font = fonts::load(&config.font_family, config.font_weight)
            .or_else(|| fonts::load("Segoe UI", config.font_weight))
            .ok_or("No usable font found")?;
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
        window::register(instance)?;
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

        let (config_tx, reports) = ping::spawn_worker(config.clone(), hwnd, WM_APP_PING)?;
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        update::spawn_checker(update_tx.clone(), hwnd, WM_APP_UPDATE);
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                instance,
                config,
                config_tx,
                reports,
                status: Status::Pending,
                history: History::default(),
                font,
                icon: None,
                update_tx,
                update_rx,
                update: None,
                notified_version: None,
                installing: false,
            })
        });
        with_app(App::add_tray);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if let Some(dialog) = window::hwnd() {
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
