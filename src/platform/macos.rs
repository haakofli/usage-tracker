//! AppKit equivalents of the Win32 behaviour in `windows.rs`: screen geometry
//! in physical pixels, cursor hit-testing, and click-through outside the card.
//!
//! Much of what the Windows backend does has no counterpart here. There is no
//! DWM border to suppress and no non-client area to collapse, because a
//! borderless `NSWindow` genuinely has no frame — so those entry points exist
//! only to keep one API across both platforms.

use anyhow::{Context, Result};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Monitor;

/// Cocoa puts the origin at the bottom-left of the primary screen with Y
/// increasing upwards. Everything else in the dock — and winit itself — uses
/// top-left with Y down, so screen rectangles are flipped about this height on
/// the way out.
fn flip_reference(mtm: MainThreadMarker) -> Option<f64> {
    let screens = NSScreen::screens(mtm);
    let primary = screens.to_vec().into_iter().next()?;
    let frame = primary.frame();
    Some(frame.origin.y + frame.size.height)
}

/// `NSScreen` reports points; the rest of the dock works in physical pixels, so
/// each screen is scaled by its own backing factor. That is also what winit
/// does, which keeps window positions and screen bounds in the same space on a
/// mixed-DPI setup.
fn monitor_from(screen: &NSScreen, flip: f64) -> Monitor {
    let frame = screen.frame();
    let scale = screen.backingScaleFactor();
    let left = (frame.origin.x * scale).round() as i32;
    let top = ((flip - (frame.origin.y + frame.size.height)) * scale).round() as i32;
    Monitor {
        left,
        top,
        right: left + (frame.size.width * scale).round() as i32,
        bottom: top + (frame.size.height * scale).round() as i32,
        name: screen.localizedName().to_string(),
    }
}

/// Every display currently attached, so a remembered one can be found again.
pub fn monitors() -> Vec<Monitor> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let Some(flip) = flip_reference(mtm) else {
        return Vec::new();
    };
    NSScreen::screens(mtm)
        .to_vec()
        .iter()
        .map(|screen| monitor_from(screen, flip))
        .collect()
}

pub struct Window {
    window: Option<Retained<NSWindow>>,
    /// Whether clicks are currently passing through, so the flag is only re-sent
    /// when it actually changes rather than on every frame.
    ignoring: Cell<bool>,
}

impl Window {
    pub fn new(handle: &impl HasWindowHandle) -> Self {
        let window = match handle.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::AppKit(h)) => {
                let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
                view.window()
            }
            _ => None,
        };

        if let Some(window) = window.as_ref() {
            // A dock belongs on every Space and beside full-screen apps.
            // Without this it vanishes the moment you switch Space, which for
            // an always-on-top quota readout defeats the point.
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            // eframe asks for these too; re-asserting them is free and means the
            // card's own rounded corners are never boxed in by AppKit's.
            window.setOpaque(false);
            window.setHasShadow(false);
        }

        Self {
            window,
            ignoring: Cell::new(false),
        }
    }

    /// No-op: a borderless `NSWindow` has no frame to strip, and nothing
    /// re-applies one behind our back the way winit does on Windows.
    pub fn keep_frameless(&self) {}

    /// No-op: the stale-border artefact this fixes is a DWM behaviour.
    pub fn refresh_border_suppression(&self) {}

    /// Bounds of the display this window is on, in **physical pixels**.
    pub fn monitor_rect_px(&self) -> Option<Monitor> {
        let mtm = MainThreadMarker::new()?;
        let screen = self.window.as_ref()?.screen()?;
        Some(monitor_from(&screen, flip_reference(mtm)?))
    }

    /// Cursor position in physical pixels relative to the window's top-left, or
    /// `None` when the pointer is outside the window.
    ///
    /// Unlike the Windows backend this does not ask who is topmost at that
    /// point. It cannot: once the pointer leaves the card, `set_hit_rect` makes
    /// the window ignore mouse events, so AppKit would report whatever is
    /// underneath and hover could never switch back on. The dock floats above
    /// everything anyway, which is what the z-order test was protecting.
    pub fn cursor_inside(&self) -> Option<(f32, f32)> {
        let window = self.window.as_ref()?;
        let frame = window.frame();
        let mouse = NSEvent::mouseLocation();

        let x = mouse.x - frame.origin.x;
        let y = (frame.origin.y + frame.size.height) - mouse.y;
        if x < 0.0 || y < 0.0 || x >= frame.size.width || y >= frame.size.height {
            return None;
        }

        let scale = window.backingScaleFactor();
        Some(((x * scale) as f32, (y * scale) as f32))
    }

    /// Restricts mouse input to the painted card.
    ///
    /// `ignoresMouseEvents` is all-or-nothing for a window, so there is no
    /// direct equivalent of answering `WM_NCHITTEST` per pixel. Testing the
    /// cursor against the card once a frame and flipping the flag reaches the
    /// same place: clicks on the transparent area around the card reach
    /// whatever is behind it, and clicks on the card do not.
    pub fn set_hit_rect(&self, l: i32, t: i32, r: i32, b: i32) {
        let inside = self
            .cursor_inside()
            .is_some_and(|(x, y)| x >= l as f32 && x < r as f32 && y >= t as f32 && y < b as f32);
        let ignore = !inside;
        if self.ignoring.get() == ignore {
            return;
        }
        self.ignoring.set(ignore);
        if let Some(window) = self.window.as_ref() {
            window.setIgnoresMouseEvents(ignore);
        }
    }

    /// Nothing to do: `set_hit_rect` already hands every event outside the card
    /// to whatever is behind the window, so there is no empty area left holding
    /// on to clicks. Windows has to cut the window down to shape instead, and
    /// the two backends present the same API.
    pub fn clip_to(&self, _l: i32, _t: i32, _r: i32, _b: i32) {}
}

/// What the zoom keys are asking for this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZoomKey {
    Bigger,
    Smaller,
    Reset,
}

/// Reads the zoom keys straight from the window server rather than through
/// egui, for the same reason as on Windows: the dock never takes keyboard
/// focus, so egui receives no key events.
///
/// `CGEventSourceKeyState` only reports whether a key is down, which needs no
/// Accessibility permission — unlike an event tap or a global `NSEvent`
/// monitor, either of which would put a permission prompt in front of a dock
/// that only wants to resize itself.
///
/// The modifier is Command here rather than Control, following the platform.
#[derive(Default)]
pub struct ZoomKeys {
    held: Option<ZoomKey>,
}

impl ZoomKeys {
    /// Returns a request only on the frame a key goes down, so holding it does
    /// not run the scale away.
    pub fn poll(&mut self) -> Option<ZoomKey> {
        // Virtual key codes from `<Carbon/HIToolbox/Events.h>`; they are layout
        // independent, so these are positions on the keyboard rather than the
        // characters a given layout produces.
        const VK_COMMAND: u16 = 0x37;
        const VK_RIGHT_COMMAND: u16 = 0x36;
        const VK_EQUAL: u16 = 0x18;
        const VK_MINUS: u16 = 0x1B;
        const VK_ZERO: u16 = 0x1D;
        const VK_KEYPAD_PLUS: u16 = 0x45;
        const VK_KEYPAD_MINUS: u16 = 0x4E;
        const VK_KEYPAD_ZERO: u16 = 0x52;

        let down =
            |key: u16| CGEventSource::key_state(CGEventSourceStateID::CombinedSessionState, key);

        if !down(VK_COMMAND) && !down(VK_RIGHT_COMMAND) {
            self.held = None;
            return None;
        }

        let now = if down(VK_EQUAL) || down(VK_KEYPAD_PLUS) {
            Some(ZoomKey::Bigger)
        } else if down(VK_MINUS) || down(VK_KEYPAD_MINUS) {
            Some(ZoomKey::Smaller)
        } else if down(VK_ZERO) || down(VK_KEYPAD_ZERO) {
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

/// Last screen layout seen, with when it was taken.
static LAST_SCREENS: Mutex<Option<(Instant, Vec<Monitor>)>> = Mutex::new(None);

/// Whether the screen layout has changed since this was last asked.
///
/// Windows announces this as a message; AppKit announces it as a notification,
/// which would mean an observer object and a delegate just to set one flag.
/// Comparing the screen list is simpler and answers the same question, so long
/// as it is not done on every frame of a 60fps hover animation — hence the
/// interval.
pub fn take_display_changed() -> bool {
    const INTERVAL: Duration = Duration::from_millis(500);

    let Ok(mut last) = LAST_SCREENS.lock() else {
        return false;
    };
    let now = Instant::now();
    if let Some((seen_at, _)) = last.as_ref()
        && now.duration_since(*seen_at) < INTERVAL
    {
        return false;
    }

    let current = monitors();
    // The first reading establishes a baseline rather than reporting a change.
    let changed = last.as_ref().is_some_and(|(_, prev)| *prev != current);
    *last = Some((now, current));
    changed
}

/// No-op: macOS themes menus from the system appearance, so a tray menu already
/// matches the dock without being asked.
pub fn use_dark_menus() {}

/// Reverse-DNS label for the login item, and the plist's file name.
const AGENT_LABEL: &str = "dev.haakofli.usage-tracker";

/// Per-user launch agents. Writing here needs no admin rights and no installer:
/// the app registers itself, and the entry is a plain readable plist the user
/// can inspect or delete.
fn agent_path() -> Result<PathBuf> {
    Ok(super::home_dir()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{AGENT_LABEL}.plist")))
}

/// Whether the dock is registered to start at login.
pub fn autostart_enabled() -> bool {
    agent_path().is_ok_and(|path| path.is_file())
}

pub fn set_autostart(on: bool) -> Result<()> {
    let path = agent_path()?;

    if !on {
        // Already absent is the desired state, not a failure.
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other.context("remove the launch agent"),
        };
    }

    let exe = std::env::current_exe().context("locate the running executable")?;
    let parent = path.parent().context("launch agent has no parent")?;
    std::fs::create_dir_all(parent).context("create ~/Library/LaunchAgents")?;

    // `launchctl load` is deliberately not shelled out to: the agent is picked
    // up at the next login either way, and spawning a process to register
    // ourselves is exactly the sort of thing this replaced an install script to
    // avoid.
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array><string>{}</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
        exe.display()
    );
    std::fs::write(&path, plist).context("write the launch agent")
}
