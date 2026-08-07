//! The Windows system tray icon and its Show/Start/Stop/Quit menu.
//!
//! `tray-icon`'s own docs (`tray-icon`'s crate-level `lib.rs`) are explicit
//! about two constraints this module exists to honor:
//!
//! 1. The [`tray_icon::TrayIcon`] itself MUST be created on the same thread
//!    that runs the platform event loop (a win32 loop on Windows), and only
//!    once that loop is actually running -- not merely constructed. `eframe`
//!    owns that loop internally; [`TrayController::new`] is called lazily
//!    from inside the app's first `logic()` call (see `main.rs`), which by
//!    construction only ever runs after `eframe`'s event loop has started.
//!    It is never called from a spawned side thread.
//! 2. Tray and menu click events must be forwarded into the running loop
//!    with an event-loop-aware wakeup, not left to be picked up only on the
//!    next scheduled repaint. `main.rs` registers a
//!    [`tray_icon::menu::MenuEvent::set_event_handler`] handler that pushes
//!    into a shared queue AND calls `egui::Context::request_repaint`, which
//!    is `eframe`'s own documented thread-safe wakeup path (it forwards to
//!    the same winit `EventLoopProxy` the crate's docs recommend using
//!    directly). `main.rs` does NOT register a
//!    [`tray_icon::TrayIconEvent::set_event_handler`] -- only the menu (the
//!    Show/Start/Stop/Quit items) drives any action, so left-clicking the
//!    icon itself does nothing beyond `tray-icon`'s own built-in
//!    menu-open behavior. This module never spawns a second event loop.
//!
//! Everything else here -- picking a color for an [`EngineState`], building
//! the tiny solid-color icon, and mapping a clicked menu item back to a
//! [`TrayCommand`] -- is ordinary, side-effect-free logic and is unit
//! tested below without ever creating a real tray icon.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::engine::EngineState;

/// Square dimension (in pixels) of the generated tray icon.
const ICON_SIZE: u32 = 32;

const SHOW_ID: &str = "show";
const START_ID: &str = "start";
const STOP_ID: &str = "stop";
const QUIT_ID: &str = "quit";

/// A user action requested from the tray menu, decoupled from any
/// `tray-icon`/`muda` type so the rest of the app never has to match on
/// [`MenuId`] strings directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Bring the main window to the front.
    Show,
    /// Launch `nunciod`.
    Start,
    /// Request a graceful `nunciod` shutdown.
    Stop,
    /// Exit the monitor (never the daemon -- see `engine`'s module docs).
    Quit,
}

/// Wraps a `muda` menu-building error (from `Menu::append`) as a
/// [`tray_icon::Error`], preserving its message rather than discarding it --
/// `menu::Menu`'s own error type does not otherwise unify with
/// `tray_icon::Error`, so [`TrayController::new`] needs this to keep
/// returning a single error type from a single fallible constructor.
fn menu_build_error(err: tray_icon::menu::Error) -> tray_icon::Error {
    tray_icon::Error::OsError(std::io::Error::other(err.to_string()))
}

/// Maps a raw menu-click event back to the [`TrayCommand`] it represents,
/// or `None` for a menu id this module did not create (there should never
/// be one, but an unrecognized id must be ignored, not panic).
#[must_use]
pub fn command_for_menu_event(event: &MenuEvent) -> Option<TrayCommand> {
    match event.id().as_ref() {
        SHOW_ID => Some(TrayCommand::Show),
        START_ID => Some(TrayCommand::Start),
        STOP_ID => Some(TrayCommand::Stop),
        QUIT_ID => Some(TrayCommand::Quit),
        _ => None,
    }
}

/// The solid RGBA color used to render the tray icon for `state`, chosen so
/// each [`EngineState`] is visually distinct at a glance: grey (stopped),
/// yellow (starting), green (running), red (not responding), dark blue
/// (unknown -- the monitor itself is broken, distinct from every daemon-side
/// state above).
#[must_use]
pub fn rgba_for_state(state: EngineState) -> [u8; 4] {
    match state {
        EngineState::Stopped => [128, 128, 128, 255],
        EngineState::Starting => [230, 200, 40, 255],
        EngineState::Running => [40, 180, 80, 255],
        EngineState::NotResponding => [210, 50, 50, 255],
        EngineState::Unknown => [60, 70, 160, 255],
    }
}

/// Builds a solid-color square [`Icon`] for `state`.
fn build_icon(state: EngineState) -> Result<Icon, tray_icon::BadIcon> {
    let [r, g, b, a] = rgba_for_state(state);
    let pixel_count = (ICON_SIZE * ICON_SIZE) as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for _ in 0..pixel_count {
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)
}

/// Owns the live tray icon and its menu items for the lifetime of the app.
///
/// Holds `tray_icon`/`muda` types directly (which are `Rc`-based and not
/// `Send`), so a `TrayController` never leaves the thread that created it --
/// exactly the GUI/event-loop thread `TrayController::new`'s own doc
/// comment requires.
pub struct TrayController {
    tray_icon: TrayIcon,
    start_item: MenuItem,
    stop_item: MenuItem,
    last_rendered_state: EngineState,
}

impl TrayController {
    /// Builds the tray icon and its Show/Start Engine/Stop Engine/Quit
    /// menu for `initial_state`.
    ///
    /// MUST be called on the thread running the platform event loop, and
    /// only after that loop has started (see this module's doc comment).
    /// `main.rs` satisfies this by calling it lazily from inside the app's
    /// first `logic()` call rather than from `eframe`'s app-creation
    /// callback or, worse, a spawned thread.
    pub fn new(initial_state: EngineState) -> tray_icon::Result<Self> {
        let show_item = MenuItem::with_id(SHOW_ID, "Show", true, None);
        let start_item =
            MenuItem::with_id(START_ID, "Start Engine", initial_state.allows_start(), None);
        let stop_item =
            MenuItem::with_id(STOP_ID, "Stop Engine", initial_state.allows_stop(), None);
        let quit_item = MenuItem::with_id(QUIT_ID, "Quit", true, None);

        let menu = Menu::new();
        menu.append(&show_item).map_err(menu_build_error)?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        menu.append(&start_item).map_err(menu_build_error)?;
        menu.append(&stop_item).map_err(menu_build_error)?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(menu_build_error)?;
        menu.append(&quit_item).map_err(menu_build_error)?;

        let icon = build_icon(initial_state)
            .map_err(|err| tray_icon::Error::OsError(std::io::Error::other(err.to_string())))?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("Nuncio Monitor")
            .build()?;

        Ok(Self {
            tray_icon,
            start_item,
            stop_item,
            last_rendered_state: initial_state,
        })
    }

    /// Updates the icon color and the Start/Stop items' enabled state for
    /// `state`, doing nothing if `state` matches what is already rendered
    /// (avoids rebuilding the icon image every frame for no reason).
    ///
    /// Returns `Some(message)` when the icon could not be rebuilt or applied
    /// -- `tracing::warn!` alone is not enough here, since this crate's
    /// `main.rs` never installs a `tracing-subscriber`-backed sink the user
    /// would see; the caller is expected to surface the message into
    /// [`crate::state::AppState::last_error`] so the failure is not silently
    /// discarded.
    #[must_use]
    pub fn set_engine_state(&mut self, state: EngineState) -> Option<String> {
        self.start_item.set_enabled(state.allows_start());
        self.stop_item.set_enabled(state.allows_stop());

        if state == self.last_rendered_state {
            return None;
        }
        self.last_rendered_state = state;
        match build_icon(state) {
            Ok(icon) => {
                if let Err(err) = self.tray_icon.set_icon(Some(icon)) {
                    tracing::warn!(error = %err, "failed to update tray icon color");
                    return Some(format!("failed to update tray icon color: {err}"));
                }
                None
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to build tray icon for new engine state");
                Some(format!(
                    "failed to build tray icon for new engine state: {err}"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tray_icon::menu::MenuId;

    fn event_for(id: &str) -> MenuEvent {
        MenuEvent {
            id: MenuId::new(id),
        }
    }

    #[test]
    fn every_known_menu_id_maps_to_its_command() {
        assert_eq!(
            command_for_menu_event(&event_for(SHOW_ID)),
            Some(TrayCommand::Show)
        );
        assert_eq!(
            command_for_menu_event(&event_for(START_ID)),
            Some(TrayCommand::Start)
        );
        assert_eq!(
            command_for_menu_event(&event_for(STOP_ID)),
            Some(TrayCommand::Stop)
        );
        assert_eq!(
            command_for_menu_event(&event_for(QUIT_ID)),
            Some(TrayCommand::Quit)
        );
    }

    #[test]
    fn an_unrecognized_menu_id_maps_to_no_command() {
        assert_eq!(command_for_menu_event(&event_for("not-ours")), None);
    }

    #[test]
    fn each_engine_state_gets_a_visually_distinct_color() {
        let colors = [
            rgba_for_state(EngineState::Stopped),
            rgba_for_state(EngineState::Starting),
            rgba_for_state(EngineState::Running),
            rgba_for_state(EngineState::NotResponding),
            rgba_for_state(EngineState::Unknown),
        ];
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(colors[i], colors[j], "states {i} and {j} share a color");
            }
        }
    }

    #[test]
    fn build_icon_produces_the_requested_square_dimensions() {
        let icon = build_icon(EngineState::Running).expect("valid rgba buffer builds an icon");
        // `Icon` does not expose width/height directly, but a successful
        // build already proves `ICON_SIZE * ICON_SIZE * 4` bytes were
        // accepted as `ICON_SIZE x ICON_SIZE` RGBA -- `Icon::from_rgba`
        // itself validates that the byte count matches the claimed
        // dimensions, so this is the failure mode actually worth guarding.
        drop(icon);
    }
}
