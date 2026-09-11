//! System tray presence: an icon, show/hide, and which providers to include.
//!
//! The dock is an undecorated, taskbar-less, always-on-top window, so the tray
//! is the only durable way to get it back once hidden.

use crate::icons;
use crate::providers::ProviderId;
use crate::settings::Settings;
use std::sync::{Arc, Mutex};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum Action {
    Toggle,
    Refresh,
    Quit,
    /// Set this provider to an explicit state.
    ///
    /// Deliberately *not* "flip it". Windows delivers the same menu command
    /// more than once, and a flip applied twice turns a provider off and
    /// straight back on — which is precisely why the checkboxes appeared to do
    /// nothing. Carrying the desired value makes re-delivery harmless.
    SetProvider(ProviderId, bool),
    /// Register or unregister the dock as a login item. Carries the desired
    /// value for the same reason as `SetProvider`.
    SetAutostart(bool),
}

struct ProviderEntry {
    id: ProviderId,
    item: CheckMenuItem,
}

/// Events pushed by the menu library's handler thread, drained on the UI
/// thread. Raw events rather than actions, because resolving one needs the menu
/// items, which stay on the UI thread.
#[derive(Default)]
struct Inbox {
    menu: Vec<MenuId>,
}

pub struct Tray {
    /// Held to keep the icon alive; dropping it removes it from the tray.
    icon: TrayIcon,
    toggle_id: MenuId,
    refresh_id: MenuId,
    quit_id: MenuId,
    autostart: CheckMenuItem,
    providers: Vec<ProviderEntry>,
    inbox: Arc<Mutex<Inbox>>,
}

impl Tray {
    /// Carries the detail the dock deliberately omits — notably each reading's
    /// observation time, which matters for Codex since its numbers are only as
    /// fresh as the last Codex request.
    pub fn set_tooltip(&self, body: &str) {
        let _ = self.icon.set_tooltip(Some(body));
    }

    pub fn new(settings: &Settings, installed: &[ProviderId], ctx: &egui::Context) -> Option<Self> {
        const SIZE: u32 = 32;
        let rgba = icons::rasterise_rgba(icons::TRAY_ICON_SVG, SIZE)?;
        let icon = Icon::from_rgba(rgba, SIZE, SIZE).ok()?;

        // Native Win32 menus honour this process-wide setting, so the tray menu
        // matches the dock instead of being a bright rectangle beside it. On
        // macOS the system already does this, and the call is a no-op.
        crate::platform::use_dark_menus();

        let refresh = MenuItem::new("Refresh now", true, None);
        let toggle = MenuItem::new("Show / hide", true, None);
        // The OS is the source of truth, so this is read rather than remembered
        // — the two cannot drift if the user removes the entry by hand.
        let autostart = CheckMenuItem::new(
            "Start at login",
            true,
            crate::platform::autostart_enabled(),
            None,
        );
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();

        // Everything found on the machine is listed, so it is obvious the dock
        // saw it. Ones with no readable quota are shown greyed rather than
        // hidden — hiding them looked identical to failing to detect them.
        // Providers that are not installed are not mentioned at all.
        let mut providers = Vec::new();
        for &id in installed {
            let readable = id.has_quota_source();
            let label = if readable {
                id.label().to_string()
            } else {
                format!("{}  (no quota to read)", id.label())
            };
            let item =
                CheckMenuItem::new(label, readable, readable && settings.is_enabled(id), None);
            menu.append(&item).ok()?;
            if readable {
                providers.push(ProviderEntry { id, item });
            }
        }
        if !installed.is_empty() {
            menu.append(&PredefinedMenuItem::separator()).ok()?;
        }

        menu.append_items(&[
            &refresh,
            &toggle,
            &autostart,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("Usage tracker — AI quota dock")
            .with_icon(icon)
            .with_icon_as_template(cfg!(target_os = "macos"))
            // Either button opens the menu; nothing is bound to a bare click.
            .with_menu_on_left_click(true)
            .with_menu(Box::new(menu))
            .build()
            .ok()?;

        // Event driven, not polled: the menu library calls these handlers when
        // something is clicked, and the repaint request wakes eframe — which
        // also works while the dock is hidden, since a repaint is what makes
        // eframe run `logic` at all.
        let inbox = Arc::new(Mutex::new(Inbox::default()));

        let menu_inbox = Arc::clone(&inbox);
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Ok(mut inbox) = menu_inbox.lock() {
                inbox.menu.push(event.id);
            }
            menu_ctx.request_repaint();
        }));

        // Clicking the icon itself does nothing: showing and hiding is an
        // explicit menu choice. Binding it to a click meant a stray click
        // could make the dock vanish with no obvious cause.
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);

        Some(Self {
            icon: tray,
            toggle_id: toggle.id().clone(),
            refresh_id: refresh.id().clone(),
            quit_id: quit.id().clone(),
            autostart,
            providers,
            inbox,
        })
    }

    /// Keeps the login-item tick in step, in case the dock's own menu changed
    /// it or the write did not take.
    pub fn sync_autostart(&self, on: bool) {
        if self.autostart.is_checked() != on {
            self.autostart.set_checked(on);
        }
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

    /// Takes whatever the event handlers have queued and turns it into actions.
    ///
    /// Runs on the UI thread, which is where the menu items live, so a check
    /// item's own tick state can be read to build an idempotent action.
    pub fn take_actions(&self) -> Vec<Action> {
        let Ok(mut inbox) = self.inbox.lock() else {
            return Vec::new();
        };
        let menu = std::mem::take(&mut inbox.menu);
        drop(inbox);

        // Windows delivers the same menu command more than once. Collapsing
        // repeats of one id within a drain makes every item duplicate-proof at
        // a stroke, rather than each action having to defend itself: a repeated
        // "Show / hide" would otherwise toggle twice and appear to do nothing,
        // exactly as the provider checkboxes did.
        let mut seen: Vec<MenuId> = Vec::new();
        let menu: Vec<MenuId> = menu
            .into_iter()
            .filter(|id| {
                let first = !seen.contains(id);
                if first {
                    seen.push(id.clone());
                }
                first
            })
            .collect();

        let mut actions = Vec::new();
        for id in menu {
            if let Some(entry) = self.providers.iter().find(|e| e.item.id() == &id) {
                // The library flips the tick before telling us, so this is the
                // state the user just asked for.
                actions.push(Action::SetProvider(entry.id, entry.item.is_checked()));
            } else if self.autostart.id() == &id {
                // The library flips the tick before telling us, so this is the
                // state the user just asked for.
                actions.push(Action::SetAutostart(self.autostart.is_checked()));
            } else if id == self.toggle_id {
                actions.push(Action::Toggle);
            } else if id == self.refresh_id {
                actions.push(Action::Refresh);
            } else if id == self.quit_id {
                actions.push(Action::Quit);
            }
        }
        actions
    }
}
