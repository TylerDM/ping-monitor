use crate::config::Config;
use std::ffi::c_void;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{GetLastError, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{
    ICMP_ECHO_REPLY, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const IP_SUCCESS: u32 = 0;
const IP_REQ_TIMED_OUT: u32 = 11010;

#[derive(Clone, Debug)]
pub enum PingError {
    Timeout,
    Failed(String),
}

pub struct Report {
    pub host: String,
    pub result: Result<u32, PingError>,
}

struct Icmp(HANDLE);

impl Icmp {
    fn new() -> windows::core::Result<Self> {
        unsafe { IcmpCreateFile().map(Self) }
    }

    /// Sends one ICMP echo and returns the round-trip time in milliseconds.
    fn echo(&self, addr: Ipv4Addr, timeout_ms: u32) -> Result<u32, PingError> {
        // Same 32-byte payload Windows' ping.exe uses.
        let payload = [b'a'; 32];
        let mut reply = vec![0u8; size_of::<ICMP_ECHO_REPLY>() + payload.len() + 256];
        let count = unsafe {
            IcmpSendEcho(
                self.0,
                u32::from_ne_bytes(addr.octets()),
                payload.as_ptr() as *const c_void,
                payload.len() as u16,
                None,
                reply.as_mut_ptr() as *mut c_void,
                reply.len() as u32,
                timeout_ms,
            )
        };
        if count == 0 {
            let code = unsafe { GetLastError() }.0;
            return Err(status_error(code));
        }
        let reply = unsafe { std::ptr::read_unaligned(reply.as_ptr() as *const ICMP_ECHO_REPLY) };
        match reply.Status {
            IP_SUCCESS => Ok(reply.RoundTripTime),
            status => Err(status_error(status)),
        }
    }
}

impl Drop for Icmp {
    fn drop(&mut self) {
        unsafe {
            let _ = IcmpCloseHandle(self.0);
        }
    }
}

fn status_error(code: u32) -> PingError {
    let message = match code {
        IP_REQ_TIMED_OUT => return PingError::Timeout,
        11002 => "Destination network unreachable".to_string(),
        11003 => "Destination host unreachable".to_string(),
        11004 => "Destination protocol unreachable".to_string(),
        11005 => "Destination port unreachable".to_string(),
        11013 => "TTL expired in transit".to_string(),
        11050 => "General failure".to_string(),
        11000..=11999 => format!("ICMP error {code}"),
        _ => windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code))
            .message()
            .trim()
            .to_string(),
    };
    PingError::Failed(message)
}

fn resolve(host: &str) -> Result<Ipv4Addr, PingError> {
    let addrs = (host, 0)
        .to_socket_addrs()
        .map_err(|e| PingError::Failed(format!("Can't resolve {host}: {e}")))?;
    addrs
        .filter_map(|a| match a {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            SocketAddr::V6(_) => None,
        })
        .next()
        .ok_or_else(|| PingError::Failed(format!("{host} has no IPv4 address")))
}

/// Starts the background ping loop. Send a new `Config` to change the target
/// or interval (applied immediately); each result is queued on the returned
/// receiver and `notify_msg` is posted to `notify` so the UI thread can drain it.
pub fn spawn_worker(
    initial: Config,
    notify: HWND,
    notify_msg: u32,
) -> std::io::Result<(Sender<Config>, Receiver<Report>)> {
    let (config_tx, config_rx) = mpsc::channel::<Config>();
    let (report_tx, report_rx) = mpsc::channel::<Report>();
    // HWND isn't Send; window handles are valid across threads, so pass the raw value.
    let notify = notify.0 as usize;

    std::thread::Builder::new()
        .name("ping".into())
        .spawn(move || {
            let icmp = Icmp::new();
            let mut config = initial;
            loop {
                let started = Instant::now();
                let timeout_ms = config.interval_ms.clamp(500, 4_000);
                let result = match &icmp {
                    Ok(icmp) => resolve(&config.host).and_then(|ip| icmp.echo(ip, timeout_ms)),
                    Err(e) => Err(PingError::Failed(format!(
                        "ICMP unavailable: {}",
                        e.message()
                    ))),
                };
                let report = Report {
                    host: config.host.clone(),
                    result,
                };
                if report_tx.send(report).is_err() {
                    return;
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(notify as *mut c_void)),
                        notify_msg,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }

                let wait = Duration::from_millis(config.interval_ms.into())
                    .saturating_sub(started.elapsed());
                match config_rx.recv_timeout(wait) {
                    Ok(new) => {
                        config = new;
                        while let Ok(newer) = config_rx.try_recv() {
                            config = newer;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })?;

    Ok((config_tx, report_rx))
}
