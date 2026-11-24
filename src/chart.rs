//! The latency / packet-loss chart on the Stats tab: a custom control drawn
//! with GDI into a back buffer, with the data lines anti-aliased by GDI+.

use crate::history::{self, Sample};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateFromHDC, GdipCreatePen1, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics,
    GdipDeletePen, GdipDrawLines, GdipFillEllipse, GdipSetPenLineJoin, GdipSetSmoothingMode,
    GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics, LineJoinRound, PointF,
    SmoothingModeAntiAlias, Status, UnitPixel,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub const CLASS_NAME: PCWSTR = w!("PingMonitorChart");

const BACKGROUND: [u8; 3] = [0xFF, 0xFF, 0xFF];
const TEXT: [u8; 3] = [0x5F, 0x5F, 0x5F];
const GRID: [u8; 3] = [0xEB, 0xEB, 0xEB];
const AXIS: [u8; 3] = [0xC8, 0xC8, 0xC8];
const LATENCY: [u8; 3] = [0x00, 0x67, 0xC0];
const LOSS: [u8; 3] = [0xC4, 0x2B, 0x1C];

thread_local! {
    static DATA: RefCell<Vec<Sample>> = const { RefCell::new(Vec::new()) };
    /// Set by the parent with WM_SETFONT; not owned here.
    static FONT: Cell<Option<HFONT>> = const { Cell::new(None) };
}

pub fn register(instance: HINSTANCE) -> windows::core::Result<()> {
    // GDI+ stays up for the life of the process; the token is never needed again.
    let mut token = 0usize;
    let input = GdiplusStartupInput {
        GdiplusVersion: 1,
        ..Default::default()
    };
    unsafe {
        GdiplusStartup(&mut token, &input, std::ptr::null_mut());
    }

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Replaces the plotted samples and repaints `chart`, if it exists.
pub fn set_data(chart: Option<HWND>, samples: Vec<Sample>) {
    DATA.replace(samples);
    if let Some(chart) = chart {
        unsafe {
            let _ = InvalidateRect(Some(chart), None, false);
        }
    }
}

fn colorref([r, g, b]: [u8; 3]) -> COLORREF {
    COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}

fn argb([r, g, b]: [u8; 3]) -> u32 {
    0xFF00_0000 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Rounds `value` up to 1, 2, 2.5 or 5 times a power of ten.
fn nice_ceil(value: f32) -> f32 {
    let magnitude = 10f32.powf(value.max(f32::MIN_POSITIVE).log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|m| m * magnitude)
        .find(|&step| step >= value)
        .unwrap_or(10.0 * magnitude)
}

struct Canvas {
    dc: HDC,
    dpi: i32,
}

impl Canvas {
    fn s(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }

    fn text_size(&self, text: &str) -> SIZE {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        unsafe {
            let _ = GetTextExtentPoint32W(self.dc, &wide, &mut size);
        }
        size
    }

    fn text(&self, x: i32, y: i32, text: &str, color: [u8; 3]) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            SetTextColor(self.dc, colorref(color));
            let _ = TextOutW(self.dc, x, y, &wide);
        }
    }

    fn line(&self, (x1, y1): (i32, i32), (x2, y2): (i32, i32), color: [u8; 3]) {
        unsafe {
            let pen = CreatePen(PS_SOLID, 1, colorref(color));
            let old = SelectObject(self.dc, pen.into());
            let _ = MoveToEx(self.dc, x1, y1, None);
            let _ = LineTo(self.dc, x2, y2);
            SelectObject(self.dc, old);
            let _ = DeleteObject(pen.into());
        }
    }

    fn fill(&self, rect: RECT, color: [u8; 3]) {
        unsafe {
            let brush = CreateSolidBrush(colorref(color));
            FillRect(self.dc, &rect, brush);
            let _ = DeleteObject(brush.into());
        }
    }
}

/// Anti-aliased polylines through GDI+.
struct Lines(*mut GpGraphics);

impl Lines {
    fn new(dc: HDC) -> Option<Self> {
        let mut graphics = std::ptr::null_mut();
        unsafe {
            if GdipCreateFromHDC(dc, &mut graphics) != Status(0) {
                return None;
            }
            GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        }
        Some(Self(graphics))
    }

    fn polyline(&self, points: &[PointF], color: [u8; 3], width: f32) {
        unsafe {
            if points.len() == 1 {
                // A lone point between losses: draw it as a dot.
                let mut brush = std::ptr::null_mut();
                GdipCreateSolidFill(argb(color), &mut brush);
                let r = width * 1.2;
                GdipFillEllipse(
                    self.0,
                    brush as *mut GpBrush,
                    points[0].X - r,
                    points[0].Y - r,
                    2.0 * r,
                    2.0 * r,
                );
                GdipDeleteBrush(brush as *mut GpBrush);
                return;
            }
            let mut pen = std::ptr::null_mut();
            GdipCreatePen1(argb(color), width, UnitPixel, &mut pen);
            GdipSetPenLineJoin(pen, LineJoinRound);
            GdipDrawLines(self.0, pen, points.as_ptr(), points.len() as i32);
            GdipDeletePen(pen);
        }
    }
}

impl Drop for Lines {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.0);
        }
    }
}

fn draw(canvas: &Canvas, width: i32, height: i32, samples: &[Sample]) {
    let s = |v| canvas.s(v);
    canvas.fill(
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
        BACKGROUND,
    );
    let line_h = canvas.text_size("0").cy;

    // Legend.
    let mut x = 0;
    let legend_y = line_h / 2;
    let legend_lines = Lines::new(canvas.dc);
    for (label, color) in [
        ("Latency (ms)", LATENCY),
        (
            &*format!("Packet loss (%, last {} pings)", history::LOSS_WINDOW),
            LOSS,
        ),
    ] {
        if let Some(lines) = &legend_lines {
            let y = legend_y as f32;
            lines.polyline(
                &[
                    PointF { X: x as f32, Y: y },
                    PointF {
                        X: (x + s(16)) as f32,
                        Y: y,
                    },
                ],
                color,
                s(2) as f32,
            );
        }
        canvas.text(x + s(22), 0, label, TEXT);
        x += s(22) + canvas.text_size(label).cx + s(20);
    }
    drop(legend_lines);

    let left = s(6) + canvas.text_size("0000").cx + s(6);
    let right = width - (s(6) + canvas.text_size("100%").cx + s(6));
    let top = line_h + s(14);
    let bottom = height - (line_h + s(8));
    let (plot_w, plot_h) = ((right - left) as f32, (bottom - top) as f32);
    if plot_w < 10.0 || plot_h < 10.0 {
        return;
    }

    // At most one point per pixel column, however many samples there are.
    let points = history::downsample(samples, plot_w as usize);
    let max_ms = points.iter().filter_map(|p| p.rtt_ms).fold(0.0, f32::max);
    // Four grid divisions, so the 0/25/50/75/100% loss axis shares the lines.
    let step = nice_ceil((max_ms * 1.05).max(20.0) / 4.0);
    let y_max = step * 4.0;
    for i in 0..=4 {
        let y = bottom - (plot_h * i as f32 / 4.0).round() as i32;
        canvas.line((left, y), (right, y), if i == 0 { AXIS } else { GRID });
        let ms = format!("{}", step * i as f32);
        canvas.text(
            left - s(6) - canvas.text_size(&ms).cx,
            y - line_h / 2,
            &ms,
            LATENCY,
        );
        canvas.text(right + s(6), y - line_h / 2, &format!("{}%", 25 * i), LOSS);
    }

    let Some(first) = samples.first() else {
        let message = "Waiting for the first ping…";
        let size = canvas.text_size(message);
        canvas.text(
            left + (plot_w as i32 - size.cx) / 2,
            top + (plot_h as i32 - size.cy) / 2,
            message,
            TEXT,
        );
        return;
    };

    // Time axis: from the oldest sample (at least a minute ago) to now.
    let now = Instant::now();
    let span = now.duration_since(first.at).max(Duration::from_secs(60));
    let x_of = |at: Instant| {
        right as f32 - plot_w * now.duration_since(at).as_secs_f32() / span.as_secs_f32()
    };
    let tick = [1u64, 2, 5, 10, 15, 30, 60, 120, 180, 240]
        .into_iter()
        .find(|&minutes| span.as_secs() / (minutes * 60) <= 6)
        .unwrap_or(240);
    let mut minutes_ago = 0;
    while minutes_ago * 60 <= span.as_secs() {
        let x =
            (right as f32 - plot_w * (minutes_ago * 60) as f32 / span.as_secs_f32()).round() as i32;
        if minutes_ago > 0 {
            canvas.line((x, top), (x, bottom), GRID);
        }
        let label = match minutes_ago {
            0 => "now".to_string(),
            m if m % 60 == 0 => format!("-{} h", m / 60),
            m => format!("-{m} min"),
        };
        let label_w = canvas.text_size(&label).cx;
        let label_x = (x - label_w / 2).min(right - label_w).max(left);
        canvas.text(label_x, bottom + s(4), &label, TEXT);
        minutes_ago += tick;
    }

    let Some(lines) = Lines::new(canvas.dc) else {
        return;
    };
    let y_ms = |ms: f32| bottom as f32 - plot_h * (ms / y_max).min(1.0);
    let y_pct = |pct: f32| bottom as f32 - plot_h * pct / 100.0;

    // Loss underneath, latency on top.
    let loss: Vec<PointF> = points
        .iter()
        .map(|point| PointF {
            X: x_of(point.at),
            Y: y_pct(point.loss_pct),
        })
        .collect();
    lines.polyline(&loss, LOSS, s(3) as f32 / 2.0);

    // Latency, broken wherever every ping was lost.
    for run in points.split(|point| point.rtt_ms.is_none()) {
        let line: Vec<PointF> = run
            .iter()
            .filter_map(|point| {
                Some(PointF {
                    X: x_of(point.at),
                    Y: y_ms(point.rtt_ms?),
                })
            })
            .collect();
        if !line.is_empty() {
            lines.polyline(&line, LATENCY, s(2) as f32);
        }
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let (width, height) = (client.right, client.bottom);

        // Draw off-screen, then copy, so live updates don't flicker.
        let mem = CreateCompatibleDC(Some(hdc));
        let bitmap = CreateCompatibleBitmap(hdc, width, height);
        let old_bitmap = SelectObject(mem, bitmap.into());
        let old_font = FONT.get().map(|font| SelectObject(mem, font.into()));
        SetBkMode(mem, TRANSPARENT);

        let canvas = Canvas {
            dc: mem,
            dpi: GetDpiForWindow(hwnd) as i32,
        };
        DATA.with_borrow(|samples| draw(&canvas, width, height, samples));

        let _ = BitBlt(hdc, 0, 0, width, height, Some(mem), 0, 0, SRCCOPY);
        if let Some(old) = old_font {
            SelectObject(mem, old);
        }
        SelectObject(mem, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            return LRESULT(0);
        }
        // Everything is painted from the back buffer.
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETFONT => {
            FONT.set(Some(HFONT(wparam.0 as *mut _)));
            return LRESULT(0);
        }
        WM_GETFONT => return LRESULT(FONT.get().map_or(0, |font| font.0 as isize)),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
