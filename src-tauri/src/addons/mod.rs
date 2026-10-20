//! Fork-local feature addons.
//!
//! Nothing in this directory exists upstream (github.com/cjpais/Handy). The
//! whole point of the layout is that upstream files stay almost untouched, so
//! `git pull upstream main` keeps being boring: every edit this fork makes to
//! an upstream file is a *single line* tagged `handy-addon:`, and everything
//! else lives in new files that can never conflict.
//!
//! Grep `handy-addon:` to find (or re-apply) the hooks. See ADDONS.md.

use tauri::AppHandle;

use crate::tray::TrayIconState;

pub mod prominent_glow;

/// Window-label prefix reserved for addon-owned windows.
///
/// Upstream's app-wide window-event handler in `lib.rs` skips any window whose
/// label starts with this, so addon windows never drag the settings window's
/// close-to-tray / activation-policy behaviour along, and never multiply the
/// per-window `ThemeChanged` tray rebuild.
const ADDON_WINDOW_PREFIX: &str = "handy_addon_";

/// Whether a window label belongs to an addon rather than to Handy proper.
pub fn owns_window(label: &str) -> bool {
    label.starts_with(ADDON_WINDOW_PREFIX)
}

/// Builds an addon window label. Labels are stable for the process lifetime —
/// addon windows are pooled and reused, never destroyed (see `prominent_glow`).
fn window_label(addon: &str, index: usize) -> String {
    format!("{ADDON_WINDOW_PREFIX}{addon}_{index}")
}

/// One-time addon setup. Called from `lib.rs::initialize_core_logic`, after the
/// managers are in Tauri state and settings have loaded.
pub fn init(app: &AppHandle) {
    prominent_glow::init(app);
}

/// Recording-lifecycle fan-out.
///
/// Hooked into `tray::set_tray_state` because that is the one function every
/// start / stop / cancel / error path funnels through, on every platform, and
/// independently of `overlay_style` and `show_tray_icon`. Hooking the overlay
/// instead would miss users who run with the overlay disabled — which is the
/// Linux default, and the likely configuration for anyone who turns the
/// prominent glow on.
pub fn on_tray_state(app: &AppHandle, state: TrayIconState) {
    prominent_glow::set_recording(app, state == TrayIconState::Recording);
}
