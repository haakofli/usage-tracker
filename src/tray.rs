//! System tray presence: an icon, show/hide, and which providers to include.
//!
//! The dock is an undecorated, taskbar-less, always-on-top window, so the tray
//! is the only durable way to get it back once hidden.

use crate::icons;
use crate::providers::ProviderId;
use crate::settings::Settings;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum Action {
    Toggle,
    Show,
    Hide,
    Quit,
    SetProvider(ProviderId, bool),
}

struct ProviderEntry {
    id: ProviderId,
    item: CheckMenuItem,
}

pub struct Tray {
    /// Held to keep the icon alive; dropping it removes it from the tray.
    icon: TrayIcon,
    toggle_id: MenuId,
    show_id: MenuId,
    hide_id: MenuId,
    quit_id: MenuId,
    providers: Vec<ProviderEntry>,
}

impl Tray {
    /// Carries the detail the dock deliberately omits — notably each reading's
    /// observation time, which matters for Codex since its numbers are only as
    /// fresh as the last Codex request.
    pub fn set_tooltip(&self, body: &str) {
        let _ = self.icon.set_tooltip(Some(body));
    }

    pub fn new(settings: &Settings, installed: &[ProviderId]) -> Option<Self> {
        const SIZE: u32 = 32;
        let rgba = icons::rasterise_rgba(icons::APP_ICON_SVG, SIZE)?;
        let icon = Icon::from_rgba(rgba, SIZE, SIZE).ok()?;

        let toggle = MenuItem::new("Show / hide", true, None);
        let show = MenuItem::new("Show", true, None);
        let hide = MenuItem::new("Hide", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append_items(&[&toggle, &PredefinedMenuItem::separator()])
            .ok()?;

        let mut providers = Vec::new();
        for &id in installed {
            let readable = id.has_quota_source();
            // Providers with no quota source are listed so it is clear the dock
            // saw them, but cannot be enabled — there is nothing to display.
            let label = if readable {
                id.label().to_string()
            } else {
                format!("{} — no quota available", id.label())
            };
            let item = CheckMenuItem::new(label, readable, settings.is_enabled(id), None);
            menu.append(&item).ok()?;
            providers.push(ProviderEntry { id, item });
        }

        menu.append_items(&[
            &PredefinedMenuItem::separator(),
            &show,
            &hide,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("Usage tracker — AI quota dock")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .ok()?;

        Some(Self {
            icon: tray,
            toggle_id: toggle.id().clone(),
            show_id: show.id().clone(),
            hide_id: hide.id().clone(),
            quit_id: quit.id().clone(),
            providers,
        })
    }

    /// Keeps the tick marks in step with the settings, in case anything other
    /// than the menu changed them.
    pub fn sync_provider_checks(&self, settings: &Settings) {
        for entry in &self.providers {
            let want = settings.is_enabled(entry.id);
            if entry.item.is_checked() != want {
                entry.item.set_checked(want);
            }
        }
    }

    /// Drains both event channels. Returns the last action requested, so a
    /// burst of clicks cannot queue up contradictory toggles.
    pub fn poll(&self) -> Option<Action> {
        let mut action = None;

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(entry) = self.providers.iter().find(|e| e.item.id() == &event.id) {
                action = Some(Action::SetProvider(entry.id, entry.item.is_checked()));
                continue;
            }
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
