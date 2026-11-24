//! Minimal HTTPS GET over WinHTTP, which uses the system's proxy settings and
//! certificate store and follows redirects.

use std::ffi::c_void;
use windows::Win32::Networking::WinHttp::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn handle(raw: *mut c_void, what: &str) -> Result<Handle, String> {
    if raw.is_null() {
        Err(format!(
            "{what} failed: {}",
            windows::core::Error::from_thread().message()
        ))
    } else {
        Ok(Handle(raw))
    }
}

/// Fetches `url` (https only) and returns the status code and body.
pub fn get(url: &str) -> Result<(u32, Vec<u8>), String> {
    let fail = |what: &str, e: windows::core::Error| format!("{what} failed: {}", e.message());
    unsafe {
        let mut host = [0u16; 256];
        let mut path = vec![0u16; 4096];
        let mut query = vec![0u16; 4096];
        let mut parts = URL_COMPONENTS {
            dwStructSize: size_of::<URL_COMPONENTS>() as u32,
            lpszHostName: PWSTR(host.as_mut_ptr()),
            dwHostNameLength: host.len() as u32,
            lpszUrlPath: PWSTR(path.as_mut_ptr()),
            dwUrlPathLength: path.len() as u32,
            lpszExtraInfo: PWSTR(query.as_mut_ptr()),
            dwExtraInfoLength: query.len() as u32,
            ..Default::default()
        };
        let wide: Vec<u16> = url.encode_utf16().collect();
        WinHttpCrackUrl(&wide, 0, &mut parts).map_err(|e| fail("Parsing the URL", e))?;
        if parts.nScheme != WINHTTP_INTERNET_SCHEME_HTTPS {
            return Err(format!("Not an https URL: {url}"));
        }
        // Path plus query string, null-terminated.
        let mut target: Vec<u16> = path[..parts.dwUrlPathLength as usize].to_vec();
        target.extend_from_slice(&query[..parts.dwExtraInfoLength as usize]);
        target.push(0);

        let agent = HSTRING::from(format!("PingMonitor/{}", env!("CARGO_PKG_VERSION")));
        let session = handle(
            WinHttpOpen(
                &agent,
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "Opening a WinHTTP session",
        )?;
        let connection = handle(
            WinHttpConnect(session.0, PCWSTR(host.as_ptr()), parts.nPort, 0),
            "Connecting",
        )?;
        let request = handle(
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "Opening the request",
        )?;

        WinHttpSendRequest(request.0, None, None, 0, 0, 0)
            .map_err(|e| fail("Sending the request", e))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| fail("Receiving the response", e))?;

        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|e| fail("Reading the status", e))?;

        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available)
                .map_err(|e| fail("Reading the response", e))?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr() as *mut c_void,
                available,
                &mut read,
            )
            .map_err(|e| fail("Reading the response", e))?;
            body.truncate(start + read as usize);
        }
        Ok((status, body))
    }
}
