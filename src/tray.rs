//! System tray presence: an icon, and a menu to show, hide or quit the dock.
//!
//! The dock is an undecorated, taskbar-less, always-on-top window, so the tray
//! is the only durable way to get it back once hidden.

use crate::icons;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum Action {
    Toggle,
    Show,
    Hide,
    Quit,
}

pub struct Tray {
    /// Held to keep the icon alive; dropping it removes it from the tray.
    _icon: TrayIcon,
    toggle_id: tray_icon::menu::MenuId,
    show_id: tray_icon::menu::MenuId,
    hide_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl Tray {
    /// Carries the detail the dock deliberately omits — notably each reading's
    /// observation time, which matters for Codex since its numbers are only as
    /// fresh as the last Codex request.
    pub fn set_tooltip(&self, body: &str) {
        let _ = self._icon.set_tooltip(Some(body));
    }

    pub fn new() -> Option<Self> {
        const SIZE: u32 = 32;
        let rgba = icons::rasterise_rgba(icons::APP_ICON_SVG, SIZE)?;
        let icon = Icon::from_rgba(rgba, SIZE, SIZE).ok()?;

        let toggle = MenuItem::new("Show / hide", true, None);
        let show = MenuItem::new("Show", true, None);
        let hide = MenuItem::new("Hide", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &toggle,
            &PredefinedMenuItem::separator(),
            &show,
            &hide,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("Usage tracker — Claude & Codex quota")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .ok()?;

        Some(Self {
            _icon: tray,
            toggle_id: toggle.id().clone(),
            show_id: show.id().clone(),
            hide_id: hide.id().clone(),
            quit_id: quit.id().clone(),
        })
    }

    /// Drains both event channels. Returns the last action requested, so a
    /// burst of clicks cannot queue up contradictory toggles.
    pub fn poll(&self) -> Option<Action> {
        let mut action = None;

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            action = if event.id == self.toggle_id {
                Some(Action::Toggle)
            } else if event.id == self.show_id {
                Some(Action::Show)
            } else if event.id == self.hide_id {
                Some(Action::Hide)
            } else if event.id == self.quit_id {
                Some(Action::Quit)
            } else {
                action
            };
        }

        // A left click on the icon is the usual way to flick the dock back.
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Down,
                ..
            } = event
            {
                action = Some(Action::Toggle);
            }
        }

        action
    }
}
