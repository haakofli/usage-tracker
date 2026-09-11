//! Win32 bits the dock needs that winit does not surface: frame suppression
//! and cursor hit-testing.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, GetCursorPos,
    GetWindowLongPtrW, GetWindowRect, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_NCCALCSIZE, WNDPROC, WS_BORDER, WS_CAPTION,
    WS_DLGFRAME, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE, WS_EX_WINDOWEDGE,
    WS_THICKFRAME, WindowFromPoint,
};

pub struct Window {
    hwnd: Option<HWND>,
}

impl Window {
    pub fn new(handle: &impl HasWindowHandle) -> Self {
        let hwnd = match handle.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => Some(HWND(isize::from(h.hwnd) as *mut _)),
            _ => None,
        };
        if let Some(hwnd) = hwnd {
            strip_frame(hwnd);
            suppress_dwm_border(hwnd);
            remove_nonclient_area(hwnd);
        }
        Self { hwnd }
    }

    /// Re-asserts the frameless styles. Cheap when nothing has changed.
    pub fn keep_frameless(&self) {
        if let Some(hwnd) = self.hwnd {
            strip_frame(hwnd);
        }
    }

    /// Re-applied after a resize, which is when the stale border reappears.
    pub fn refresh_border_suppression(&self) {
        if let Some(hwnd) = self.hwnd {
            suppress_dwm_border(hwnd);
        }
    }

    /// Cursor position in physical pixels relative to the window's top-left,
    /// but only when this window is the one actually under the cursor.
    ///
    /// Hover is read from the cursor rather than from egui's enter/leave
    /// events on purpose: window changes emit `WM_MOUSELEAVE`, so driving the
    /// expansion from those events lets the resize feed back into the hover
    /// state and oscillate. `WindowFromPoint` also respects z-order, so a
    /// window covering the dock correctly counts as "not hovering".
    pub fn cursor_inside(&self) -> Option<(f32, f32)> {
        let hwnd = self.hwnd?;
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).ok()?;
            if WindowFromPoint(p) != hwnd {
                return None;
            }
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).ok()?;
            Some(((p.x - r.left) as f32, (p.y - r.top) as f32))
        }
    }
}

/// Original window procedure, kept so the subclass can forward to it. The dock
/// only ever has one window, so a single slot is enough.
static ORIGINAL_WNDPROC: AtomicIsize = AtomicIsize::new(0);

/// Collapses the non-client area to nothing.
///
/// Windows keeps a one-pixel top frame for a borderless window so it still
/// snaps and animates, and DWM paints that frame — a white hairline along the
/// top edge, which survived clearing every `WS_*` and `WS_EX_*` style and
/// setting `DWMWA_BORDER_COLOR` to none. Answering `WM_NCCALCSIZE` with zero
/// makes the client area cover the whole window, so there is no frame left to
/// paint. Every other message goes to winit untouched.
fn remove_nonclient_area(hwnd: HWND) {
    unsafe {
        let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_proc as *const () as isize);
        if previous != 0 {
            ORIGINAL_WNDPROC.store(previous, Ordering::SeqCst);
        }
        // Force the frame to be recalculated with the new handler in place.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // wparam != 0 means the client rect is being proposed; returning 0 accepts
    // the whole window as client area.
    if msg == WM_NCCALCSIZE && wparam.0 != 0 {
        return LRESULT(0);
    }
    let original = ORIGINAL_WNDPROC.load(Ordering::SeqCst);
    if original == 0 {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    unsafe {
        CallWindowProcW(
            Some(std::mem::transmute::<isize, WNDPROC>(original).unwrap()),
            hwnd,
            msg,
            wparam,
            lparam,
        )
    }
}

/// Turns off the Windows 11 window border and DWM's own corner rounding.
///
/// Separate from the `WS_*` styles: DWM draws this border itself, so clearing
/// window styles does not remove it. It showed as a pure-white hairline above
/// the card, and after a resize a stale segment of it survived at the previous
/// window width — measured at exactly 123px, the collapsed width, while the
/// window was expanded. `DWMWA_COLOR_NONE` removes it outright.
fn suppress_dwm_border(hwnd: HWND) {
    /// `DWMWA_COLOR_NONE` — documented sentinel meaning "no border at all".
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    const DONOTROUND: i32 = 1; // DWMWCP_DONOTROUND

    unsafe {
        let none = COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::addr_of!(none).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        // The card draws its own corners; DWM rounding on top would clip them.
        let corner = DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::addr_of!(corner).cast(),
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Clears every frame and edge style from the window.
///
/// `WS_EX_WINDOWEDGE` is the one that actually shows: it draws a raised
/// hairline right on the window boundary, measured at RGB(63,68,69) against a
/// RGB(34,43,45) wallpaper — the stray light outline around the dock. The
/// `WS_*` frame bits are cleared alongside it since an undecorated window has
/// no use for them either.
///
/// Called every frame: winit re-applies styles on resize, and this is a cheap
/// read-and-compare that only touches the window when something crept back.
pub fn strip_frame(hwnd: HWND) {
    const STYLE_MASK: u32 = WS_CAPTION.0 | WS_THICKFRAME.0 | WS_BORDER.0 | WS_DLGFRAME.0;
    const EX_MASK: u32 =
        WS_EX_WINDOWEDGE.0 | WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0 | WS_EX_DLGMODALFRAME.0;

    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let new_style = style & !STYLE_MASK;
        let new_ex = ex & !EX_MASK;
        if new_style == style && new_ex == ex {
            return;
        }
        SetWindowLongPtrW(hwnd, GWL_STYLE, new_style as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex as isize);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}
