//! Re-paste chord: hold the other side's twin of a one-modifier transcribe key
//! (Left Command for Right Command), then press the transcribe key, and Handy
//! pastes the last transcript again. The press never reaches the coordinator.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ::handy_keys::{Hotkey, Modifiers};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_store::StoreExt;

use crate::input::EnigoState;
use crate::managers::history::HistoryManager;
use crate::settings::{get_settings, AppSettings, ClipboardHandling, PasteMethod};
use crate::tray::TrayIconState;

/// Frontend -> backend: "send me the current config".
const EVENT_GET: &str = "handy-addon:repaste-chord:get";
/// Frontend -> backend: "apply and persist this config".
const EVENT_SET: &str = "handy-addon:repaste-chord:set";
/// Backend -> frontend: the current config.
const EVENT_STATE: &str = "handy-addon:repaste-chord:state";

/// Shared with the glow; `portable::store_path` keeps portable installs honest.
const STORE_FILE: &str = "addons_store.json";
const STORE_KEY: &str = "repaste_chord";

const TRANSCRIBE: &str = "transcribe";

/// Pasted texts remembered, newest last.
const MAX_OUTPUTS: usize = 2;
/// How long the re-paste waits for both keys to come up before pasting anyway.
const KEYS_UP_WAIT: Duration = Duration::from_secs(2);
const KEYS_UP_POLL: Duration = Duration::from_millis(10);

/// One-modifier transcribe key, its twin's device bit, and both sides' device
/// bits. Device bits are the NX_DEVICE*KEYMASK values in IOLLEvent.h.
const TWINS: [(Modifiers, u64, u64); 8] = [
    (Modifiers::CMD_RIGHT, 0x0008, 0x0018),
    (Modifiers::CMD_LEFT, 0x0010, 0x0018),
    (Modifiers::OPT_RIGHT, 0x0020, 0x0060),
    (Modifiers::OPT_LEFT, 0x0040, 0x0060),
    (Modifiers::SHIFT_RIGHT, 0x0002, 0x0006),
    (Modifiers::SHIFT_LEFT, 0x0004, 0x0006),
    (Modifiers::CTRL_RIGHT, 0x0001, 0x2001),
    (Modifiers::CTRL_LEFT, 0x2000, 0x2001),
];

#[cfg(target_os = "macos")]
mod hid {
    use std::ffi::c_void;

    /// kCGEventSourceStateHIDSystemState: the hardware's view of the keyboard.
    const HID_SYSTEM_STATE: i32 = 1;
    /// kCGHIDEventTap, where enigo posts too.
    const HID_EVENT_TAP: u32 = 0;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
        fn CGEventCreateKeyboardEvent(
            source: *const c_void,
            keycode: u16,
            key_down: bool,
        ) -> *mut c_void;
        fn CGEventSetFlags(event: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *mut c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    pub fn device_flags() -> u64 {
        // SAFETY: CGEventSourceFlagsState only reads the current modifier state.
        unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) }
    }

    /// Taps one key carrying `flags`. No modifier event is posted, so a
    /// physically held Command stays held for the system and the next chord.
    pub fn tap_key(keycode: u16, flags: u64) -> bool {
        for key_down in [true, false] {
            // SAFETY: a null source is allowed; the event is released below.
            let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), keycode, key_down) };
            if event.is_null() {
                return false;
            }
            // SAFETY: `event` is a valid CGEventRef owned by this function.
            unsafe {
                CGEventSetFlags(event, flags);
                CGEventPost(HID_EVENT_TAP, event);
                CFRelease(event);
            }
        }
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod hid {
    pub fn device_flags() -> u64 {
        0
    }

    pub fn tap_key(_keycode: u16, _flags: u64) -> bool {
        false
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct RepasteConfig {
    pub enabled: bool,
}

impl Default for RepasteConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// In-memory config; the shortcut path must never touch the Tauri store.
static CONFIG: Mutex<RepasteConfig> = Mutex::new(RepasteConfig { enabled: true });

/// Mirrors the tray: true while nothing is recording or transcribing.
static PIPELINE_IDLE: AtomicBool = AtomicBool::new(true);

struct ChordState {
    /// Press time of a consumed chord press whose release is still pending.
    held: Option<Instant>,
    /// The last two pasted texts, newest last.
    outputs: Vec<(Instant, String)>,
}

static STATE: Mutex<ChordState> = Mutex::new(ChordState {
    held: None,
    outputs: Vec::new(),
});

/// Callers must drop the guard before calling anything that locks it again.
fn lock_state() -> MutexGuard<'static, ChordState> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn config() -> RepasteConfig {
    *CONFIG.lock().unwrap_or_else(|p| p.into_inner())
}

fn load_config(app: &AppHandle) -> RepasteConfig {
    let Ok(store) = app.store(crate::portable::store_path(STORE_FILE)) else {
        warn!("repaste-chord: could not open {STORE_FILE}; using defaults");
        return RepasteConfig::default();
    };
    let Some(value) = store.get(STORE_KEY) else {
        return RepasteConfig::default();
    };
    serde_json::from_value::<RepasteConfig>(value).unwrap_or_else(|error| {
        warn!("repaste-chord: stored config is unreadable ({error}); using defaults");
        RepasteConfig::default()
    })
}

fn save_config(app: &AppHandle, config: RepasteConfig) {
    let Ok(store) = app.store(crate::portable::store_path(STORE_FILE)) else {
        warn!("repaste-chord: could not open {STORE_FILE}; config not persisted");
        return;
    };
    match serde_json::to_value(config) {
        Ok(value) => {
            store.set(STORE_KEY, value);
            // Auto-save is debounced; save now so a hard quit cannot lose it.
            if let Err(error) = store.save() {
                warn!("repaste-chord: failed to save {STORE_FILE}: {error}");
            }
        }
        Err(error) => warn!("repaste-chord: failed to serialize config: {error}"),
    }
}

fn broadcast_state(app: &AppHandle, config: RepasteConfig) {
    let _ = app.emit_to("main", EVENT_STATE, config);
}

/// One-time setup: restore the persisted config and wire the settings-UI bridge.
pub fn init(app: &AppHandle) {
    let initial = load_config(app);
    *CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = initial;
    debug!("repaste-chord: loaded {initial:?}");

    let handle = app.clone();
    app.listen(EVENT_GET, move |_event| {
        broadcast_state(&handle, config());
    });

    let handle = app.clone();
    app.listen(EVENT_SET, move |event| {
        let next = match serde_json::from_str::<RepasteConfig>(event.payload()) {
            Ok(config) => config,
            Err(error) => {
                warn!("repaste-chord: ignoring malformed config payload: {error}");
                return;
            }
        };
        *CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = next;
        save_config(&handle, next);
        broadcast_state(&handle, next);
    });
}

/// The twin's device bit and both sides' bits, for a one-modifier transcribe key.
fn twin_masks(transcribe: &str) -> Option<(u64, u64)> {
    let hotkey: Hotkey = transcribe.parse().ok()?;
    if hotkey.key.is_some() {
        return None;
    }
    TWINS
        .iter()
        .find(|(key, _, _)| *key == hotkey.modifiers)
        .map(|&(_, twin, both)| (twin, both))
}

/// Sees every shortcut event before upstream does. Returns true when the event
/// is consumed and must not reach the coordinator.
pub fn intercept_input(
    app: &AppHandle,
    binding_id: &str,
    hotkey_string: &str,
    is_pressed: bool,
) -> bool {
    if binding_id != TRANSCRIBE {
        return false;
    }
    if is_pressed {
        on_press(hotkey_string)
    } else {
        on_release(app, hotkey_string)
    }
}

/// Tray state fan-out from `addons::on_tray_state`.
pub fn on_tray_state(state: TrayIconState) {
    PIPELINE_IDLE.store(state == TrayIconState::Idle, Ordering::SeqCst);
}

fn on_press(hotkey: &str) -> bool {
    let chord = config().enabled
        && PIPELINE_IDLE.load(Ordering::SeqCst)
        && twin_masks(hotkey).is_some_and(|(twin, _)| hid::device_flags() & twin != 0);
    // handy-keys never repeats a held key, so this also clears a lost release.
    lock_state().held = chord.then(Instant::now);
    if chord {
        debug!("repaste-chord: chord pressed; re-paste on release");
    }
    chord
}

fn on_release(app: &AppHandle, hotkey: &str) -> bool {
    let pressed_at = lock_state().held.take();
    let Some(pressed_at) = pressed_at else {
        return false;
    };
    let (twin, both) = twin_masks(hotkey).unwrap_or((0, 0));
    let handle = app.clone();
    std::thread::spawn(move || {
        // Twin still down: paste now, once per tap, without touching Command.
        if hid::device_flags() & twin != 0 {
            let settings = get_settings(&handle);
            if held_paste_supported(&settings) {
                let app = handle.clone();
                let _ = handle
                    .run_on_main_thread(move || repaste_held(&app, pressed_at, twin, &settings));
                return;
            }
        }
        // The synthetic Cmd+V must not overlap a physically held modifier.
        let deadline = Instant::now() + KEYS_UP_WAIT;
        while hid::device_flags() & both != 0 && Instant::now() < deadline {
            std::thread::sleep(KEYS_UP_POLL);
        }
        repaste(&handle, pressed_at);
    });
    true
}

/// Records a text about to be pasted. Called from `actions.rs` just before the
/// real paste; re-pastes skip it, so they do not shift the remembered outputs.
pub fn on_paste(text: &str) {
    let mut state = lock_state();
    state.outputs.push((Instant::now(), text.to_string()));
    let excess = state.outputs.len().saturating_sub(MAX_OUTPUTS);
    if excess > 0 {
        state.outputs.drain(..excess);
    }
}

/// Newest output older than `before`, so a paste racing the chord is skipped.
fn pick_output(outputs: &[(Instant, String)], before: Instant) -> Option<&str> {
    outputs
        .iter()
        .rev()
        .find(|(at, _)| *at < before)
        .map(|(_, text)| text.as_str())
}

/// The newest output before the press, else the newest history entry.
fn repaste_text(app: &AppHandle, before: Instant) -> Option<String> {
    let remembered = {
        let state = lock_state();
        pick_output(&state.outputs, before).map(str::to_string)
    };
    let text = remembered.or_else(|| latest_history_text(app))?;
    if text.trim().is_empty() {
        info!("repaste-chord: last transcript is empty; nothing to re-paste");
        return None;
    }
    Some(text)
}

/// The held paste re-implements only the plain Cmd+V clipboard path.
/// Auto-submit would land as Cmd+Return under the held Command.
fn held_paste_supported(settings: &AppSettings) -> bool {
    settings.paste_method == PasteMethod::CtrlV && !settings.auto_submit && !settings.reliable_paste
}

/// Re-pastes under the held twin by tapping only V: a synthetic Command-up from
/// `utils::paste` would clear the held key, so the next tap would start a recording.
/// Main thread only: it resolves the layout's V and serializes rapid taps.
fn repaste_held(app: &AppHandle, before: Instant, twin: u64, settings: &AppSettings) {
    let Some(text) = repaste_text(app, before) else {
        return;
    };
    let text = if settings.append_trailing_space {
        format!("{text} ")
    } else {
        text
    };

    let clipboard = app.clipboard();
    let saved_text = clipboard.read_text().ok().filter(|t| !t.is_empty());
    let saved_image = if saved_text.is_none() {
        clipboard.read_image().ok().map(|image| image.to_owned())
    } else {
        None
    };
    if let Err(e) = clipboard.write_text(text) {
        error!("repaste-chord: clipboard write failed: {e}");
        return;
    }
    std::thread::sleep(Duration::from_millis(settings.paste_delay_ms));

    #[cfg(target_os = "macos")]
    crate::input::log_paste_target();
    let flags = hid::device_flags();
    let (sent, keystroke) = if flags & twin != 0 {
        (
            hid::tap_key(command_v_keycode(), flags),
            "V under held Command",
        )
    } else {
        // Released during the delay, so a full synthetic Cmd+V is safe again.
        let sent = app.try_state::<EnigoState>().is_some_and(|enigo| {
            let mut enigo = enigo.0.lock().unwrap_or_else(|p| p.into_inner());
            crate::input::send_paste_ctrl_v(&mut enigo, 100).is_ok()
        });
        (sent, "full Cmd+V")
    };

    std::thread::sleep(Duration::from_millis(settings.paste_delay_after_ms));
    if settings.clipboard_handling != ClipboardHandling::CopyToClipboard {
        let _ = match (saved_text, saved_image) {
            (Some(text), _) => clipboard.write_text(text),
            (None, Some(image)) => clipboard.write_image(&image),
            (None, None) => clipboard.clear(),
        };
    }

    if sent {
        info!("repaste-chord: re-pasted last transcript ({keystroke})");
    } else {
        error!("repaste-chord: could not send the paste keystroke");
        let _ = app.emit("paste-error", ());
    }
}

#[cfg(target_os = "macos")]
fn command_v_keycode() -> u16 {
    crate::input::command_v_keycode()
}

#[cfg(not(target_os = "macos"))]
fn command_v_keycode() -> u16 {
    0
}

fn repaste(app: &AppHandle, before: Instant) {
    let Some(text) = repaste_text(app, before) else {
        return;
    };

    let handle = app.clone();
    let _ = app.run_on_main_thread(move || match crate::utils::paste(text, handle.clone()) {
        Ok(()) => info!("repaste-chord: re-pasted last transcript"),
        Err(e) => {
            error!("repaste-chord: paste failed: {e}");
            let _ = handle.emit("paste-error", ());
        }
    });
}

fn latest_history_text(app: &AppHandle) -> Option<String> {
    let history = app.try_state::<Arc<HistoryManager>>()?;
    match history.get_latest_completed_entry() {
        Ok(Some(entry)) => Some(
            entry
                .post_processed_text
                .as_deref()
                .unwrap_or(&entry.transcription_text)
                .to_string(),
        ),
        Ok(None) => {
            info!("repaste-chord: history is empty; nothing to re-paste");
            None
        }
        Err(e) => {
            error!("repaste-chord: could not read history: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn right_command_pairs_with_left_command() {
        assert_eq!(twin_masks("command_right"), Some((0x0008, 0x0018)));
        assert_eq!(twin_masks("command_left"), Some((0x0010, 0x0018)));
        assert_eq!(twin_masks("option_right"), Some((0x0020, 0x0060)));
    }

    #[test]
    fn shortcuts_without_a_twin_have_no_chord() {
        // Either-side modifiers, combos, and keyed shortcuts have no other side.
        assert_eq!(twin_masks("command"), None);
        assert_eq!(twin_masks("command_right+shift_right"), None);
        assert_eq!(twin_masks("option+space"), None);
        assert_eq!(twin_masks("f5"), None);
        assert_eq!(twin_masks(""), None);
    }

    fn outputs(base: Instant) -> Vec<(Instant, String)> {
        vec![
            (base + ms(10), "older".to_string()),
            (base + ms(100), "newer".to_string()),
        ]
    }

    #[test]
    fn pick_output_takes_the_newest_when_both_are_older() {
        let base = Instant::now();
        assert_eq!(pick_output(&outputs(base), base + ms(200)), Some("newer"));
    }

    #[test]
    fn pick_output_skips_an_output_newer_than_the_press() {
        let base = Instant::now();
        assert_eq!(pick_output(&outputs(base), base + ms(50)), Some("older"));
    }

    #[test]
    fn pick_output_is_none_when_all_are_newer_or_empty() {
        let base = Instant::now();
        assert_eq!(pick_output(&outputs(base), base + ms(10)), None);
        assert_eq!(pick_output(&[], Instant::now()), None);
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let config: RepasteConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, RepasteConfig::default());
        let config: RepasteConfig = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!config.enabled);
    }
}
