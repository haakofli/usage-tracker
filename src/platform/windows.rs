//! Win32 bits the dock needs that winit does not surface: frame suppression
//! and cursor hit-testing.

use anyhow::{Context, Result};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS,
    REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, GetCursorPos,
    GetWindowLongPtrW, GetWindowRect, HTTRANSPARENT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_NCCALCSIZE, WM_NCHITTEST, WM_NCPAINT, WM_SETTINGCHANGE, WNDPROC, WS_BORDER, WS_CAPTION,
    WS_DLGFRAME, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE, WS_EX_WINDOWEDGE,
    WS_THICKFRAME, WindowFromPoint,
};
use windows::core::{BOOL, PCSTR, PCWSTR, w};

use super::Monitor;

/// Where Windows looks for per-user programs to launch at sign-in. Writing here
/// needs no admin rights and no installer: the app registers itself, and the
/// entry shows up in Task Manager's Startup tab where the user can audit or
/// disable it.
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("usage-tracker");

fn open_run_key(access: REG_SAM_FLAGS) -> Option<HKEY> {
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
    status.is_ok().then_some(key)
}

/// Opens the Run key for writing, creating it when absent.
///
/// A profile that has never had a startup program does not have this key at
/// all, and `RegOpenKeyExW` fails outright rather than creating one — so
/// registering the dock on a fresh Windows install failed until this did.
fn create_run_key() -> Option<HKEY> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    status.is_ok().then_some(key)
}

/// Whether the dock is registered to start at sign-in.
pub fn autostart_enabled() -> bool {
    let Some(key) = open_run_key(KEY_READ) else {
        return false;
    };
    let present = unsafe { RegQueryValueExW(key, RUN_VALUE, None, None, None, None) }.is_ok();
    let _ = unsafe { RegCloseKey(key) };
    present
}

pub fn set_autostart(on: bool) -> Result<()> {
    let key = create_run_key().context("open the Run key for writing")?;

    let outcome = if on {
        let exe = std::env::current_exe().context("locate the running executable")?;
        // Quoted, so a path with spaces in it still launches.
        let value: Vec<u16> = format!("\"{}\"", exe.display())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let bytes =
            unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2) };
        unsafe { RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes)) }.ok()
    } else {
        // Already absent is the desired state, not a failure.
        match unsafe { RegDeleteValueW(key, RUN_VALUE) } {
            e if e == ERROR_FILE_NOT_FOUND => Ok(()),
            e => e.ok(),
        }
    };

    let _ = unsafe { RegCloseKey(key) };
    outcome.context("update the Run key")
}

unsafe fn monitor_info(handle: HMONITOR) -> Option<Monitor> {
    unsafe {
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let ptr = std::ptr::addr_of_mut!(info).cast::<MONITORINFO>();
        if !GetMonitorInfoW(handle, ptr).as_bool() {
            return None;
        }
        let r = info.monitorInfo.rcMonitor;
        let end = info
            .szDevice
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.szDevice.len());
        Some(Monitor {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
            name: String::from_utf16_lossy(&info.szDevice[..end]),
        })
    }
}

/// Every display currently attached, so a remembered one can be found again.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        handle: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let out = data.0 as *mut Vec<Monitor>;
        if let Some(info) = unsafe { monitor_info(handle) } {
            unsafe { (*out).push(info) };
        }
        BOOL(1)
    }

    let mut found: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(std::ptr::addr_of_mut!(found) as isize),
        );
    }
    found
}

pub struct Window {
    hwnd: Option<HWND>,
    /// The region last handed to Windows, so an unchanged one is not re-sent.
    clip: Cell<(i32, i32, i32, i32)>,
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
        Self {
            hwnd,
            clip: Cell::new((0, 0, 0, 0)),
        }
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

    /// Bounds of the monitor this window is on, in **physical pixels**.
    ///
    /// Asked of Win32 rather than egui on purpose. `ViewportInfo::monitor_size`
    /// is in zoom-inclusive points *and* lags a frame behind a zoom change, so
    /// anchoring to it drifted further from the screen edge with every zoom
    /// step. Physical pixels do not move when the zoom does.
    pub fn monitor_rect_px(&self) -> Option<Monitor> {
        let hwnd = self.hwnd?;
        unsafe { monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)) }
    }

    /// Cursor position in physical pixels relative to the window's top-left,
    /// but only when this window is the one actually under the cursor.
    ///
    /// Hover is read from the cursor rather than from egui's enter/leave
    /// events on purpose: window changes emit `WM_MOUSELEAVE`, so driving the
    /// expansion from those events lets it feed back into the hover state and
    /// oscillate. `WindowFromPoint` also honours both z-order and the hit test
    /// below, so the transparent area around the card correctly reads as "not
    /// hovering".
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

    /// Restricts mouse input to the painted card.
    ///
    /// The window is deliberately kept at its expanded size at all times —
    /// resizing it mid-animation recreates the GL surface and makes the hover
    /// stutter — so most of it is transparent while collapsed. Answering
    /// `WM_NCHITTEST` with `HTTRANSPARENT` outside the card keeps the shadow
    /// band from reading as a hover and from taking a click, but it is not on
    /// its own enough to let another application have that click — see
    /// [`Window::clip_to`].
    ///
    /// The rectangle is read by the hit test on the message pump rather than
    /// applied here, so this is just four stores.
    pub fn set_hit_rect(&self, l: i32, t: i32, r: i32, b: i32) {
        HIT_L.store(l, Ordering::Relaxed);
        HIT_T.store(t, Ordering::Relaxed);
        HIT_R.store(r, Ordering::Relaxed);
        HIT_B.store(b, Ordering::Relaxed);
    }

    /// Cuts the window down to the card and the room its shadow needs, so the
    /// transparent expanse beside a collapsed rail is not part of the window at
    /// all.
    ///
    /// `HTTRANSPARENT` was doing this job and cannot: Win32 only forwards a hit
    /// test answered that way to other windows *on the same thread*, so a click
    /// aimed at another application landed on the dock's own empty space and
    /// went nowhere. `WindowFromPoint` honours it regardless of thread, which
    /// is why hovering read correctly while clicking did not. A window region
    /// is applied by the window manager itself and holds for every process.
    ///
    /// It clips painting as well as input, which is why it is given the
    /// shadow's margin rather than just the card.
    pub fn clip_to(&self, l: i32, t: i32, r: i32, b: i32) {
        if self.clip.get() == (l, t, r, b) {
            return;
        }
        let Some(hwnd) = self.hwnd else {
            return;
        };
        unsafe {
            let region = CreateRectRgn(l, t, r, b);
            // Windows owns the region once it accepts it, and only then.
            if SetWindowRgn(hwnd, Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
                return;
            }
        }
        self.clip.set((l, t, r, b));
    }
}

/// What the zoom keys are asking for this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZoomKey {
    Bigger,
    Smaller,
    Reset,
}

/// Reads the zoom keys straight from the keyboard rather than through egui.
///
/// The dock never takes keyboard focus — it would steal it from whatever you
/// are actually working in — so egui receives no key events. Callers gate this
/// on the pointer being over the dock, which keeps Ctrl +/- working normally
/// everywhere else.
#[derive(Default)]
pub struct ZoomKeys {
    held: Option<ZoomKey>,
}

impl ZoomKeys {
    /// Returns a request only on the frame a key goes down, so holding it does
    /// not run the scale away.
    pub fn poll(&mut self) -> Option<ZoomKey> {
        const VK_CONTROL: i32 = 0x11;
        const VK_OEM_PLUS: i32 = 0xBB;
        const VK_OEM_MINUS: i32 = 0xBD;
        const VK_ADD: i32 = 0x6B;
        const VK_SUBTRACT: i32 = 0x6D;
        const VK_0: i32 = 0x30;
        const VK_NUMPAD0: i32 = 0x60;

        let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };

        if !down(VK_CONTROL) {
            self.held = None;
            return None;
        }

        let now = if down(VK_OEM_PLUS) || down(VK_ADD) {
            Some(ZoomKey::Bigger)
        } else if down(VK_OEM_MINUS) || down(VK_SUBTRACT) {
            Some(ZoomKey::Smaller)
        } else if down(VK_0) || down(VK_NUMPAD0) {
            Some(ZoomKey::Reset)
        } else {
            None
        };

        let fired = match (self.held, now) {
            (prev, Some(k)) if prev != Some(k) => Some(k),
            _ => None,
        };
        self.held = now;
        fired
    }
}

/// Original window procedure, kept so the subclass can forward to it. The dock
/// only ever has one window, so a single slot is enough.
static ORIGINAL_WNDPROC: AtomicIsize = AtomicIsize::new(0);

/// Set when the displays are rearranged, so the dock can re-attach itself.
static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

/// Whether the screen layout has changed since this was last asked.
///
/// Plugging in a monitor moves the window and invalidates the screen bounds it
/// was anchored to, but nothing in the normal frame loop notices — the dock was
/// left floating in the middle of a display. Windows announces this, so it is
/// worth listening for rather than comparing bounds every frame.
pub fn take_display_changed() -> bool {
    DISPLAY_CHANGED.swap(false, Ordering::Relaxed)
}

/// The painted card, in physical pixels relative to the window's top-left.
/// Read by the hit test on the UI thread's own message pump.
static HIT_L: AtomicI32 = AtomicI32::new(0);
static HIT_T: AtomicI32 = AtomicI32::new(0);
static HIT_R: AtomicI32 = AtomicI32::new(i32::MAX);
static HIT_B: AtomicI32 = AtomicI32::new(i32::MAX);

/// Switches Win32 menus — which is what the tray menu is — to dark mode.
///
/// There is no documented API for this. `uxtheme.dll` exports
/// `SetPreferredAppMode` and `FlushMenuThemes` by ordinal only (135 and 136),
/// which is what shell apps use to get dark context menus. Both calls are
/// entirely optional: if the ordinals ever move, the menu simply stays light
/// rather than the app failing.
pub fn use_dark_menus() {
    const SET_PREFERRED_APP_MODE: u16 = 135;
    const FLUSH_MENU_THEMES: u16 = 136;
    /// `PreferredAppMode::ForceDark`
    const FORCE_DARK: i32 = 2;

    unsafe {
        let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else {
            return;
        };
        if let Some(addr) = GetProcAddress(uxtheme, PCSTR(SET_PREFERRED_APP_MODE as usize as _)) {
            let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(addr);
            set_mode(FORCE_DARK);
        }
        if let Some(addr) = GetProcAddress(uxtheme, PCSTR(FLUSH_MENU_THEMES as usize as _)) {
            let flush: extern "system" fn() = std::mem::transmute(addr);
            flush();
        }
    }
}

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

    // Swallow non-client painting, so no frame is drawn even if Windows decides
    // one is due — which is what put a title bar over the dock after the tray
    // menu changed the window's activation state.
    //
    // `WM_NCACTIVATE` is deliberately *not* intercepted. Doing so broke
    // dragging outright: eframe only honours `StartDrag` when
    // `window.has_focus()`, and short-circuiting activation meant the window
    // never gained focus. Returning TRUE there does not suppress the frame
    // either — per the Win32 docs it asks for default processing — so it cost
    // dragging and bought nothing.
    if msg == WM_NCPAINT {
        return LRESULT(0);
    }

    // Note and forward: the monitor layout or this window's DPI changed, so
    // whatever the dock was anchored to no longer holds.
    if msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED || msg == WM_SETTINGCHANGE {
        DISPLAY_CHANGED.store(true, Ordering::Relaxed);
    }

    if msg == WM_NCHITTEST {
        // lparam carries screen coordinates in its two 16-bit halves.
        let x = (lparam.0 & 0xFFFF) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        let mut r = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut r) }.is_ok() {
            let (lx, ly) = (x - r.left, y - r.top);
            let inside = lx >= HIT_L.load(Ordering::Relaxed)
                && lx < HIT_R.load(Ordering::Relaxed)
                && ly >= HIT_T.load(Ordering::Relaxed)
                && ly < HIT_B.load(Ordering::Relaxed);
            if !inside {
                return LRESULT(HTTRANSPARENT as isize);
            }
        }
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
