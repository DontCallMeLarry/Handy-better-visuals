//! "Prominent" recording indicator.
//!
//! While recording, every attached display gets a full-screen, click-through,
//! always-on-top window painting a soft orange vignette in from all four edges.
//! It replaces (or accompanies) the small pill overlay for people who want an
//! unmissable "the mic is live" signal across a multi-monitor desk.
//!
//! Deliberate design constraints, all in service of a small upstream diff:
//!
//! * **Own settings file.** Config lives in `addons_store.json`, not in
//!   `AppSettings`, so `settings.rs`, `bindings.ts` and `settingsStore.ts` are
//!   untouched. `bindings.ts` alone is a generated 1100-line file that upstream
//!   rewrites constantly — adding one field there would mean a conflict on
//!   almost every pull.
//! * **Events, not commands.** The settings UI talks to this module over the
//!   Tauri event bus (`app.listen` receives frontend `emit`s), so no command
//!   goes into `lib.rs`'s `collect_commands!` and no TypeScript is regenerated.
//! * **A static page, not a Vite entry.** The glow document is
//!   `public/prominent-glow.html`, which Vite copies to the dist root verbatim.
//!   That keeps `vite.config.ts` untouched. The page has no Tauri JS API access
//!   (addon window labels are in no capability, by design) — Rust drives it via
//!   `eval`, which is not ACL-gated.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use log::{debug, warn};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener};
use tauri_plugin_store::StoreExt;

#[cfg(target_os = "macos")]
mod macos;
mod surface;

/// Frontend -> backend: "send me the current config".
const EVENT_GET: &str = "handy-addon:prominent-glow:get";
/// Frontend -> backend: "apply and persist this config".
const EVENT_SET: &str = "handy-addon:prominent-glow:set";
/// Backend -> frontend: the current config.
const EVENT_STATE: &str = "handy-addon:prominent-glow:state";

/// Separate from `settings_store.json` so upstream's settings read/write path
/// can never race or clobber addon state. `portable::store_path` is mandatory:
/// without it, portable installs would silently read a different file than the
/// rest of the app (settings.rs does the same).
const STORE_FILE: &str = "addons_store.json";
const STORE_KEY: &str = "prominent_glow";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct GlowConfig {
    /// Off by default: this is an opt-in fork feature, and a full-screen tint
    /// is not something to spring on someone who just pulled an update.
    pub enabled: bool,
    /// Peak alpha at the very screen edge. The vignette reaches zero well
    /// before the middle of the screen at any value, so this is "how loud",
    /// not "how much screen is covered".
    pub intensity: f64,
}

const DEFAULT_INTENSITY: f64 = 0.55;
const MIN_INTENSITY: f64 = 0.15;
const MAX_INTENSITY: f64 = 1.0;

impl Default for GlowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: DEFAULT_INTENSITY,
        }
    }
}

impl GlowConfig {
    fn sanitized(mut self) -> Self {
        // A NaN from a hand-edited store would otherwise reach CSS as "NaN"
        // and blank the whole gradient.
        if !self.intensity.is_finite() {
            self.intensity = DEFAULT_INTENSITY;
        }
        self.intensity = self.intensity.clamp(MIN_INTENSITY, MAX_INTENSITY);
        self
    }
}

/// In-memory config. `set_recording` runs on the audio/shortcut path, so it
/// must not touch the Tauri store — this cache is the only thing it reads.
static CONFIG: Mutex<GlowConfig> = Mutex::new(GlowConfig {
    enabled: false,
    intensity: DEFAULT_INTENSITY,
});

/// Whether the app is currently recording, tracked independently of the glow's
/// visibility so that enabling the setting mid-recording can light it up and
/// disabling it mid-recording can tear it down.
static RECORDING: AtomicBool = AtomicBool::new(false);

fn config() -> GlowConfig {
    CONFIG.lock().map(|c| *c).unwrap_or_default()
}

fn load_config(app: &AppHandle) -> GlowConfig {
    let Ok(store) = app.store(crate::portable::store_path(STORE_FILE)) else {
        warn!("prominent-glow: could not open {STORE_FILE}; using defaults");
        return GlowConfig::default();
    };
    let Some(value) = store.get(STORE_KEY) else {
        return GlowConfig::default();
    };
    match serde_json::from_value::<GlowConfig>(value) {
        Ok(config) => config.sanitized(),
        Err(error) => {
            warn!("prominent-glow: stored config is unreadable ({error}); using defaults");
            GlowConfig::default()
        }
    }
}

fn save_config(app: &AppHandle, config: GlowConfig) {
    let Ok(store) = app.store(crate::portable::store_path(STORE_FILE)) else {
        warn!("prominent-glow: could not open {STORE_FILE}; config not persisted");
        return;
    };
    match serde_json::to_value(config) {
        Ok(value) => {
            store.set(STORE_KEY, value);
            // The plugin's auto-save is debounced; a hard quit right after a
            // toggle would otherwise lose it.
            if let Err(error) = store.save() {
                warn!("prominent-glow: failed to save {STORE_FILE}: {error}");
            }
        }
        Err(error) => warn!("prominent-glow: failed to serialize config: {error}"),
    }
}

fn broadcast_state(app: &AppHandle, config: GlowConfig) {
    // Targeted at the settings window: the glow windows have no JS listeners
    // and the overlay does not care.
    let _ = app.emit_to("main", EVENT_STATE, config);
}

/// One-time setup: restore the persisted config, wire the settings-UI event
/// bridge, and pre-build the window pool if the feature is already on.
pub fn init(app: &AppHandle) {
    // Named `initial`, not `config`: a local binding would shadow the
    // module-level `fn config()` for the whole body, and the EVENT_GET listener
    // below must answer with the *current* config, not the startup snapshot.
    let initial = load_config(app);
    if let Ok(mut guard) = CONFIG.lock() {
        *guard = initial;
    }
    debug!("prominent-glow: loaded {initial:?}");

    // `emit` from the frontend reaches Rust listeners as well as other
    // webviews, so this is the whole IPC surface — no commands required.
    let handle = app.clone();
    app.listen(EVENT_GET, move |_event| {
        broadcast_state(&handle, config());
    });

    let handle = app.clone();
    app.listen(EVENT_SET, move |event| {
        let next = match serde_json::from_str::<GlowConfig>(event.payload()) {
            Ok(config) => config.sanitized(),
            Err(error) => {
                warn!("prominent-glow: ignoring malformed config payload: {error}");
                return;
            }
        };
        apply_config(&handle, next);
    });

    if initial.enabled {
        // Building a webview window costs tens of milliseconds; doing it now
        // keeps it off the keypress-to-glow path later.
        surface::ensure_pool(app);
    }
}

fn apply_config(app: &AppHandle, next: GlowConfig) {
    let previous = config();
    if let Ok(mut guard) = CONFIG.lock() {
        *guard = next;
    }
    save_config(app, next);
    broadcast_state(app, next);

    if next.enabled && !previous.enabled {
        surface::ensure_pool(app);
        // Turning it on mid-recording should light up immediately, not on the
        // next recording.
        if RECORDING.load(Ordering::Relaxed) {
            surface::show(app, next.intensity);
            // The recording can end between the load above and `show` marking
            // the surfaces visible, in which case the concurrent `hide` sees
            // nothing to do and the glow would stay up. Re-check after the
            // fact; `hide` is a no-op if it is already down.
            if !RECORDING.load(Ordering::Relaxed) {
                surface::hide(app);
            }
        }
    } else if !next.enabled && previous.enabled {
        // Windows are pooled, never destroyed — hiding is the teardown.
        surface::hide(app);
    } else if next.enabled && RECORDING.load(Ordering::Relaxed) {
        // Live intensity preview while dragging the slider during a recording.
        surface::set_intensity(app, next.intensity);
    }
}

/// Recording started or stopped. Called for every lifecycle transition via
/// `addons::on_tray_state`; idempotent, because tray state is also re-applied
/// on theme changes.
pub fn set_recording(app: &AppHandle, recording: bool) {
    if RECORDING.swap(recording, Ordering::Relaxed) == recording {
        return;
    }

    let config = config();
    if !config.enabled {
        return;
    }

    if recording {
        surface::show(app, config.intensity);
    } else {
        surface::hide(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_is_clamped_into_the_usable_range() {
        assert_eq!(
            GlowConfig {
                enabled: true,
                intensity: 4.0
            }
            .sanitized()
            .intensity,
            MAX_INTENSITY
        );
        assert_eq!(
            GlowConfig {
                enabled: true,
                intensity: -1.0
            }
            .sanitized()
            .intensity,
            MIN_INTENSITY
        );
    }

    #[test]
    fn non_finite_intensity_falls_back_to_the_default() {
        assert_eq!(
            GlowConfig {
                enabled: true,
                intensity: f64::NAN
            }
            .sanitized()
            .intensity,
            DEFAULT_INTENSITY
        );
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let config: GlowConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, GlowConfig::default());

        // A partial payload from an older frontend must not reset the rest.
        let config: GlowConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(config.enabled);
        assert_eq!(config.intensity, DEFAULT_INTENSITY);
    }
}
