//! macOS glow surface: one borderless, non-activating `NSPanel` per display.
//!
//! Kept in its own module because `tauri_panel!` emits module-scope `use`
//! items, and `overlay.rs` already invokes it once — a second invocation in the
//! same module would be a duplicate-import error.
//!
//! The builder call deliberately mirrors `overlay::create_recording_overlay`
//! so this stays on the exact configuration upstream already ships and tests.

use tauri::{AppHandle, WebviewUrl};
use tauri_nspanel::{
    tauri_panel, CollectionBehavior, ManagerExt, PanelBuilder, PanelLevel, StyleMask,
};

tauri_panel! {
    panel!(ProminentGlowPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Creates one hidden glow panel. Main thread only.
pub fn create_panel(app: &AppHandle, label: &str, url: &str) -> Result<(), String> {
    PanelBuilder::<_, ProminentGlowPanel>::new(app, label)
        .url(WebviewUrl::App(url.into()))
        .title("Recording indicator")
        // Real geometry is applied by `surface::place` before every show; a
        // 1x1 placeholder keeps the pre-placement frame off-screen-small
        // instead of flashing a full white rectangle.
        .position(tauri::Position::Logical(tauri::LogicalPosition {
            x: 0.0,
            y: 0.0,
        }))
        .size(tauri::Size::Logical(tauri::LogicalSize {
            width: 1.0,
            height: 1.0,
        }))
        // Status (25) clears the menu bar (24) and the Dock (20), so the glow
        // reaches the true screen edge on all sides.
        .level(PanelLevel::Status)
        .has_shadow(false)
        .transparent(true)
        // Click-through, applied synchronously during build(). Tauri's
        // `set_ignore_cursor_events` reaches the same selector but is
        // dispatched async, which would leave the first frame swallowing
        // clicks across the entire screen.
        .ignores_mouse_events(true)
        .no_activate(true)
        // `borderless()` assigns rather than ORs, so it must come first or it
        // wipes the non-activating bit. Same order as overlay.rs. With no
        // Titled bit, AppKit skips constrainFrameRect:toScreen: and the panel
        // can actually cover the menu bar.
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .with_window(|w| {
            w.decorations(false)
                .transparent(true)
                .focusable(false)
                .focused(false)
                .shadow(false)
                .skip_taskbar(true)
                .visible(false)
        })
        .build()
        .map(|panel| {
            // Built visible would flash a full-screen rectangle at startup.
            panel.hide();
        })
        .map_err(|error| error.to_string())
}

/// `orderFrontRegardless` — raises without ever activating the app.
pub fn show_panel(app: &AppHandle, label: &str) {
    match app.get_webview_panel(label) {
        Ok(panel) => panel.show(),
        // tauri_nspanel::Error is Debug-only, so no Display formatting here.
        Err(error) => log::warn!("prominent-glow: panel {label} unavailable: {error:?}"),
    }
}

/// `orderOut:`. The panel is kept alive and reused — never closed. Upstream's
/// app-wide `CloseRequested` handler calls `prevent_close()` and flips the
/// activation policy, so closing an addon window would both fail to destroy it
/// and make Handy's Dock icon disappear.
pub fn hide_panel(app: &AppHandle, label: &str) {
    if let Ok(panel) = app.get_webview_panel(label) {
        panel.hide();
    }
}
