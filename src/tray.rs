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
    Refresh,
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
    refresh_id: MenuId,
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
        let rgba = icons::rasterise_rgba(icons::TRAY_ICON_SVG, SIZE)?;
        let icon = Icon::from_rgba(rgba, SIZE, SIZE).ok()?;

        // Native Win32 menus honour this process-wide setting, so the tray menu
        // matches the dock instead of being a bright rectangle beside it.
        crate::winshape::use_dark_menus();

        let refresh = MenuItem::new("Refresh now", true, None);
        let toggle = MenuItem::new("Show / hide", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();

        // Only providers the dock can actually read are listed. Something
        // installed but unreadable is not an option worth offering, and
        // anything not installed is not mentioned at all.
        let mut providers = Vec::new();
        let usable: Vec<_> = installed
            .iter()
            .copied()
            .filter(|id| id.has_quota_source())
            .collect();

        for &id in &usable {
            let item = CheckMenuItem::new(id.label(), true, settings.is_enabled(id), None);
            menu.append(&item).ok()?;
            providers.push(ProviderEntry { id, item });
        }
        if !usable.is_empty() {
            menu.append(&PredefinedMenuItem::separator()).ok()?;
        }

        menu.append_items(&[&refresh, &toggle, &PredefinedMenuItem::separator(), &quit])
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
            refresh_id: refresh.id().clone(),
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
            } else if event.id == self.refresh_id {
                Some(Action::Refresh)
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
