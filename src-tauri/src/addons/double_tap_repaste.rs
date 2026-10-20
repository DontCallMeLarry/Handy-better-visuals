//! Double-tap re-paste: two quick taps on a transcribe key, starting from idle,
//! re-paste the last transcript. In toggle mode a press still starts and the
//! next press still stops; only a second tap landing within the window of the
//! first is taken over. The first tap's stub recording is cancelled, the second
//! press and its release never reach the coordinator, and the re-paste fires on
//! that release. Same event-bus plumbing as `prominent_glow`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_store::StoreExt;

use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::tray::TrayIconState;

/// Frontend -> backend: "send me the current config".
const EVENT_GET: &str = "handy-addon:double-tap-repaste:get";
/// Frontend -> backend: "apply and persist this config".
const EVENT_SET: &str = "handy-addon:double-tap-repaste:set";
/// Backend -> frontend: the current config.
const EVENT_STATE: &str = "handy-addon:double-tap-repaste:state";

/// Shared with the glow; `portable::store_path` keeps portable installs honest.
const STORE_FILE: &str = "addons_store.json";
const STORE_KEY: &str = "double_tap_repaste";

const DEFAULT_WINDOW_MS: u64 = 350;
const MIN_WINDOW_MS: u64 = 150;
const MAX_WINDOW_MS: u64 = 600;

/// A press held longer than this is a hold, not a tap. Protects push-to-talk
/// and hold-or-toggle users who say one word and press again at once.
const TAP_MAX: Duration = Duration::from_millis(200);
/// A release followed by a press sooner than this is an X11 auto-repeat pair
/// (about 1 ms apart), not a human re-press.
const MIN_GAP: Duration = Duration::from_millis(25);
/// Pasted texts remembered, newest last.
const MAX_OUTPUTS: usize = 2;
/// How long after a double-tap a stub recording that started late is still
/// caught. Covers a slow (Bluetooth) microphone open.
const STUB_WATCH: Duration = Duration::from_secs(3);
const STUB_POLL: Duration = Duration::from_millis(50);

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct RepasteConfig {
    pub enabled: bool,
    /// Longest gap between the first release and the second press.
    pub window_ms: u64,
}

impl Default for RepasteConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_ms: DEFAULT_WINDOW_MS,
        }
    }
}

impl RepasteConfig {
    fn sanitized(mut self) -> Self {
        self.window_ms = self.window_ms.clamp(MIN_WINDOW_MS, MAX_WINDOW_MS);
        self
    }
}

/// In-memory config; the shortcut path must never touch the Tauri store.
static CONFIG: Mutex<RepasteConfig> = Mutex::new(RepasteConfig {
    enabled: true,
    window_ms: DEFAULT_WINDOW_MS,
});

/// Mirrors the tray: true while nothing is recording or transcribing. Fed by
/// `addons::on_tray_state`, so reading it on the input thread takes no lock.
static PIPELINE_IDLE: AtomicBool = AtomicBool::new(true);

/// Transcribe presses forwarded to the coordinator. Lets the stub watchdog
/// tell a late stub from a recording the user started on purpose.
static FORWARDED_PRESSES: AtomicU64 = AtomicU64::new(0);

/// A press that found the pipeline idle, so it is the one that starts the
/// stub recording a second tap would cancel.
struct Tap {
    binding: String,
    pressed_at: Instant,
    released_at: Option<Instant>,
}

/// The second tap of a double-tap, still held down.
struct SecondTap {
    binding: String,
    /// The first tap's press. Anything pasted at or after it is the stub's.
    first_pressed_at: Instant,
}

#[derive(Debug, PartialEq, Eq)]
enum Press {
    /// Not ours: the coordinator handles it as usual.
    Forward,
    /// Second tap of a double-tap: consume it and cancel the first tap's stub.
    DoubleTap,
}

struct TapState {
    last_tap: Option<Tap>,
    second_tap: Option<SecondTap>,
    /// The last two pasted texts, newest last.
    outputs: Vec<(Instant, String)>,
}

impl TapState {
    /// Pure so the gesture rules are unit-testable without an `AppHandle`.
    fn press(
        &mut self,
        binding: &str,
        now: Instant,
        cfg: RepasteConfig,
        pipeline_idle: bool,
    ) -> Press {
        // handy-keys never repeats a held key, so a second press of the same
        // binding means the second tap's release was lost. Drop it rather than
        // eat this press too.
        if self
            .second_tap
            .as_ref()
            .is_some_and(|s| s.binding == binding)
        {
            self.second_tap = None;
        }
        match self.last_tap.take() {
            Some(prev) if classify_press(Some(&prev), binding, now, cfg) => {
                self.second_tap = Some(SecondTap {
                    binding: binding.to_string(),
                    first_pressed_at: prev.pressed_at,
                });
                Press::DoubleTap
            }
            _ => {
                // A toggle stop press, or a press while transcribing, can never
                // open a double-tap: the press after it stays upstream's.
                self.last_tap = pipeline_idle.then(|| Tap {
                    binding: binding.to_string(),
                    pressed_at: now,
                    released_at: None,
                });
                Press::Forward
            }
        }
    }

    /// The first tap's press time when this release ends a double-tap. The
    /// caller then consumes the release and re-pastes.
    fn release(&mut self, binding: &str, now: Instant) -> Option<Instant> {
        if self
            .second_tap
            .as_ref()
            .is_some_and(|s| s.binding == binding)
        {
            return self.second_tap.take().map(|s| s.first_pressed_at);
        }
        if let Some(tap) = self
            .last_tap
            .as_mut()
            .filter(|t| t.binding == binding && t.released_at.is_none())
        {
            tap.released_at = Some(now);
        }
        None
    }
}

static STATE: Mutex<TapState> = Mutex::new(TapState {
    last_tap: None,
    second_tap: None,
    outputs: Vec::new(),
});

fn lock_state() -> MutexGuard<'static, TapState> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn config() -> RepasteConfig {
    *CONFIG.lock().unwrap_or_else(|p| p.into_inner())
}

fn load_config(app: &AppHandle) -> RepasteConfig {
    let Ok(store) = app.store(crate::portable::store_path(STORE_FILE)) else {
        warn!("double-tap-repaste: could not open {STORE_FILE}; using defaults");
        return RepasteConfig::default();
    };
    let Some(value) = store.get(STORE_KEY) else {
        return RepasteConfig::default();
    };
    match serde_json::from_value::<RepasteConfig>(value) {
        Ok(config) => config.sanitized(),
        Err(error) => {
            warn!("double-tap-repaste: stored config is unreadable ({error}); using defaults");
            RepasteConfig::default()
        }
    }
}

fn save_config(app: &AppHandle, config: RepasteConfig) {
    let Ok(store) = app.store(crate::portable::store_path(STORE_FILE)) else {
        warn!("double-tap-repaste: could not open {STORE_FILE}; config not persisted");
        return;
    };
    match serde_json::to_value(config) {
        Ok(value) => {
            store.set(STORE_KEY, value);
            // Auto-save is debounced; save now so a hard quit cannot lose it.
            if let Err(error) = store.save() {
                warn!("double-tap-repaste: failed to save {STORE_FILE}: {error}");
            }
        }
        Err(error) => warn!("double-tap-repaste: failed to serialize config: {error}"),
    }
}

fn broadcast_state(app: &AppHandle, config: RepasteConfig) {
    let _ = app.emit_to("main", EVENT_STATE, config);
}

/// One-time setup: restore the persisted config and wire the settings-UI bridge.
pub fn init(app: &AppHandle) {
    let initial = load_config(app);
    *CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = initial;
    debug!("double-tap-repaste: loaded {initial:?}");

    let handle = app.clone();
    app.listen(EVENT_GET, move |_event| {
        broadcast_state(&handle, config());
    });

    let handle = app.clone();
    app.listen(EVENT_SET, move |event| {
        let next = match serde_json::from_str::<RepasteConfig>(event.payload()) {
            Ok(config) => config.sanitized(),
            Err(error) => {
                warn!("double-tap-repaste: ignoring malformed config payload: {error}");
                return;
            }
        };
        *CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = next;
        save_config(&handle, next);
        broadcast_state(&handle, next);
    });
}

/// Whether a press of `binding` at `now` is the second tap of a double-tap.
///
/// Pure so the timing rules are unit-testable without an `AppHandle`.
fn classify_press(prev: Option<&Tap>, binding: &str, now: Instant, cfg: RepasteConfig) -> bool {
    if !cfg.enabled {
        return false;
    }
    let Some(prev) = prev else {
        return false;
    };
    if prev.binding != binding {
        return false;
    }
    // No release between the presses means key-repeat, not a tap.
    let Some(released_at) = prev.released_at else {
        return false;
    };
    if released_at.saturating_duration_since(prev.pressed_at) > TAP_MAX {
        return false;
    }
    let gap = now.saturating_duration_since(released_at);
    gap >= MIN_GAP && gap < Duration::from_millis(cfg.window_ms)
}

/// Newest output older than `before`. Anything at or after `before` came from
/// the first tap's own stub recording and must not be re-pasted.
fn pick_output(outputs: &[(Instant, String)], before: Instant) -> Option<&str> {
    outputs
        .iter()
        .rev()
        .find(|(at, _)| *at < before)
        .map(|(_, text)| text.as_str())
}

/// Sees every transcribe-binding event before the coordinator does. Returns
/// true when the event is consumed and must not reach the coordinator.
pub fn intercept_input(app: &AppHandle, binding_id: &str, is_pressed: bool) -> bool {
    let now = Instant::now();
    if is_pressed {
        on_press(app, binding_id, now)
    } else {
        on_release(app, binding_id, now)
    }
}

/// Tray state fan-out from `addons::on_tray_state`.
pub fn on_tray_state(state: TrayIconState) {
    PIPELINE_IDLE.store(state == TrayIconState::Idle, Ordering::SeqCst);
}

fn on_press(app: &AppHandle, binding: &str, now: Instant) -> bool {
    let cfg = config();
    let idle = PIPELINE_IDLE.load(Ordering::SeqCst);
    let press = lock_state().press(binding, now, cfg, idle);
    match press {
        Press::Forward => {
            FORWARDED_PRESSES.fetch_add(1, Ordering::SeqCst);
            false
        }
        Press::DoubleTap => {
            debug!("double-tap-repaste: double-tap on '{binding}'");
            cancel_stub(app);
            true
        }
    }
}

fn on_release(app: &AppHandle, binding: &str, now: Instant) -> bool {
    let Some(first_pressed_at) = lock_state().release(binding, now) else {
        return false;
    };
    // Paste on the release, so a modifier shortcut such as Right Command is
    // up again before the synthetic Cmd+V.
    repaste(app, first_pressed_at);
    true
}

/// Cancels the recording the first tap started from idle. Runs off the input
/// thread, because the cancel waits on a microphone that is still opening.
fn cancel_stub(app: &AppHandle) {
    let handle = app.clone();
    let presses = FORWARDED_PRESSES.load(Ordering::SeqCst);
    std::thread::spawn(move || {
        crate::utils::cancel_current_operation(&handle);
        // A cancel that reached the recorder before the coordinator did lets
        // the stub start afterwards, with the coordinator already idle. Without
        // this the mic would stay open with nothing able to stop it.
        let deadline = Instant::now() + STUB_WATCH;
        while Instant::now() < deadline {
            std::thread::sleep(STUB_POLL);
            if FORWARDED_PRESSES.load(Ordering::SeqCst) != presses {
                // The user pressed again; any recording now is theirs.
                return;
            }
            let recording = handle
                .try_state::<Arc<AudioRecordingManager>>()
                .is_some_and(|rm| rm.is_recording());
            if recording {
                warn!("double-tap-repaste: stub recording started after the cancel; cancelling it");
                crate::utils::cancel_current_operation(&handle);
                return;
            }
        }
    });
}

/// Records a text about to be pasted. Called from `actions.rs` just before the
/// real paste; `repaste` itself does not call it, so re-pastes do not shift the
/// remembered outputs.
pub fn on_paste(text: &str) {
    let mut state = lock_state();
    state.outputs.push((Instant::now(), text.to_string()));
    let excess = state.outputs.len().saturating_sub(MAX_OUTPUTS);
    if excess > 0 {
        state.outputs.drain(..excess);
    }
}

enum Source {
    Memory(String),
    /// Outputs exist but all came from the first tap's own recording.
    TooNew,
    /// Fresh launch: nothing pasted yet this session.
    History,
}

fn repaste(app: &AppHandle, before: Instant) {
    let source = {
        let state = lock_state();
        if state.outputs.is_empty() {
            Source::History
        } else {
            match pick_output(&state.outputs, before) {
                Some(text) => Source::Memory(text.to_string()),
                None => Source::TooNew,
            }
        }
    };

    let text = match source {
        Source::Memory(text) => text,
        Source::TooNew => {
            info!("double-tap-repaste: only output is from the first tap; nothing to re-paste");
            return;
        }
        Source::History => match latest_history_text(app) {
            Some(text) => text,
            None => return,
        },
    };

    if text.trim().is_empty() {
        info!("double-tap-repaste: last transcript is empty; nothing to re-paste");
        return;
    }

    let handle = app.clone();
    let _ = app.run_on_main_thread(move || match crate::utils::paste(text, handle.clone()) {
        Ok(()) => info!("double-tap-repaste: re-pasted last transcript"),
        Err(e) => {
            error!("double-tap-repaste: paste failed: {e}");
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
            info!("double-tap-repaste: history is empty; nothing to re-paste");
            None
        }
        Err(e) => {
            error!("double-tap-repaste: could not read history: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIBE: &str = "transcribe";

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn tap(binding: &str, pressed_at: Instant, released_at: Option<Instant>) -> Tap {
        Tap {
            binding: binding.to_string(),
            pressed_at,
            released_at,
        }
    }

    /// A tap pressed at `base`, held `hold_ms`; returns it and its release time.
    fn held(base: Instant, hold_ms: u64) -> (Tap, Instant) {
        let released = base + ms(hold_ms);
        (tap(TRANSCRIBE, base, Some(released)), released)
    }

    #[test]
    fn quick_tap_then_repress_inside_window_is_a_double_tap() {
        let base = Instant::now();
        let (prev, released) = held(base, 80);
        assert!(classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(120),
            RepasteConfig::default()
        ));
    }

    #[test]
    fn held_press_is_not_a_tap() {
        let base = Instant::now();
        let (prev, released) = held(base, 350);
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(30),
            RepasteConfig::default()
        ));
    }

    #[test]
    fn tap_max_boundary() {
        let base = Instant::now();
        let (prev, released) = held(base, 200);
        assert!(classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(100),
            RepasteConfig::default()
        ));
        let (prev, released) = held(base, 201);
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(100),
            RepasteConfig::default()
        ));
    }

    #[test]
    fn repress_after_the_window_is_not_a_double_tap() {
        let base = Instant::now();
        let (prev, released) = held(base, 80);
        let cfg = RepasteConfig::default();
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(cfg.window_ms),
            cfg
        ));
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(cfg.window_ms + 1),
            cfg
        ));
        assert!(classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(cfg.window_ms - 1),
            cfg
        ));
    }

    #[test]
    fn repress_just_after_release_is_an_x11_repeat_pair() {
        let base = Instant::now();
        let (prev, released) = held(base, 80);
        let cfg = RepasteConfig::default();
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(5),
            cfg
        ));
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(24),
            cfg
        ));
        assert!(classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(25),
            cfg
        ));
    }

    #[test]
    fn different_binding_is_not_a_double_tap() {
        let base = Instant::now();
        let (prev, released) = held(base, 80);
        assert!(!classify_press(
            Some(&prev),
            "transcribe_with_post_process",
            released + ms(120),
            RepasteConfig::default()
        ));
    }

    #[test]
    fn disabled_is_never_a_double_tap() {
        let base = Instant::now();
        let (prev, released) = held(base, 80);
        let cfg = RepasteConfig {
            enabled: false,
            ..RepasteConfig::default()
        };
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            released + ms(120),
            cfg
        ));
    }

    #[test]
    fn unreleased_previous_press_is_key_repeat() {
        let base = Instant::now();
        let prev = tap(TRANSCRIBE, base, None);
        assert!(!classify_press(
            Some(&prev),
            TRANSCRIBE,
            base + ms(120),
            RepasteConfig::default()
        ));
    }

    #[test]
    fn no_previous_tap_is_not_a_double_tap() {
        assert!(!classify_press(
            None,
            TRANSCRIBE,
            Instant::now(),
            RepasteConfig::default()
        ));
    }

    fn idle_state() -> TapState {
        TapState {
            last_tap: None,
            second_tap: None,
            outputs: Vec::new(),
        }
    }

    #[test]
    fn toggle_start_then_stop_is_never_a_double_tap() {
        let mut state = idle_state();
        let cfg = RepasteConfig::default();
        let base = Instant::now();
        assert_eq!(state.press(TRANSCRIBE, base, cfg, true), Press::Forward);
        assert_eq!(state.release(TRANSCRIBE, base + ms(80)), None);
        // Speak, then press to stop: well past the window.
        assert_eq!(
            state.press(TRANSCRIBE, base + ms(3_000), cfg, false),
            Press::Forward
        );
        assert_eq!(state.release(TRANSCRIBE, base + ms(3_080)), None);
    }

    #[test]
    fn double_tap_from_idle_repastes_on_the_second_release() {
        let mut state = idle_state();
        let cfg = RepasteConfig::default();
        let base = Instant::now();
        assert_eq!(state.press(TRANSCRIBE, base, cfg, true), Press::Forward);
        assert_eq!(state.release(TRANSCRIBE, base + ms(80)), None);
        // The first tap's stub has the pipeline busy by now.
        assert_eq!(
            state.press(TRANSCRIBE, base + ms(200), cfg, false),
            Press::DoubleTap
        );
        assert_eq!(state.release(TRANSCRIBE, base + ms(280)), Some(base));
        // The gesture is spent: the next quick press is an ordinary one.
        assert_eq!(
            state.press(TRANSCRIBE, base + ms(400), cfg, true),
            Press::Forward
        );
    }

    #[test]
    fn quick_press_after_a_toggle_stop_stays_upstreams() {
        let mut state = idle_state();
        let cfg = RepasteConfig::default();
        let base = Instant::now();
        // The stop press finds a recording, so it cannot open a double-tap.
        assert_eq!(state.press(TRANSCRIBE, base, cfg, false), Press::Forward);
        assert_eq!(state.release(TRANSCRIBE, base + ms(80)), None);
        assert_eq!(
            state.press(TRANSCRIBE, base + ms(200), cfg, false),
            Press::Forward
        );
    }

    #[test]
    fn lost_second_release_does_not_eat_the_next_press() {
        let mut state = idle_state();
        let cfg = RepasteConfig::default();
        let base = Instant::now();
        state.press(TRANSCRIBE, base, cfg, true);
        state.release(TRANSCRIBE, base + ms(80));
        assert_eq!(
            state.press(TRANSCRIBE, base + ms(200), cfg, false),
            Press::DoubleTap
        );
        // Release never arrives. The next press records as usual.
        assert_eq!(
            state.press(TRANSCRIBE, base + ms(5_000), cfg, true),
            Press::Forward
        );
        assert_eq!(state.release(TRANSCRIBE, base + ms(5_080)), None);
    }

    #[test]
    fn other_bindings_release_does_not_end_the_second_tap() {
        let mut state = idle_state();
        let cfg = RepasteConfig::default();
        let base = Instant::now();
        state.press(TRANSCRIBE, base, cfg, true);
        state.release(TRANSCRIBE, base + ms(80));
        state.press(TRANSCRIBE, base + ms(200), cfg, false);
        assert_eq!(
            state.release("transcribe_with_post_process", base + ms(250)),
            None
        );
        assert_eq!(state.release(TRANSCRIBE, base + ms(280)), Some(base));
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
    fn pick_output_skips_an_output_newer_than_the_first_tap() {
        let base = Instant::now();
        assert_eq!(pick_output(&outputs(base), base + ms(50)), Some("older"));
    }

    #[test]
    fn pick_output_is_none_when_all_are_newer() {
        let base = Instant::now();
        assert_eq!(pick_output(&outputs(base), base + ms(5)), None);
        assert_eq!(pick_output(&outputs(base), base + ms(10)), None);
    }

    #[test]
    fn pick_output_is_none_when_empty() {
        assert_eq!(pick_output(&[], Instant::now()), None);
    }

    #[test]
    fn window_is_clamped_into_the_usable_range() {
        let low = RepasteConfig {
            enabled: true,
            window_ms: 0,
        };
        let high = RepasteConfig {
            enabled: true,
            window_ms: 60_000,
        };
        assert_eq!(low.sanitized().window_ms, MIN_WINDOW_MS);
        assert_eq!(high.sanitized().window_ms, MAX_WINDOW_MS);
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let config: RepasteConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, RepasteConfig::default());
        assert!(config.enabled);
        assert_eq!(config.window_ms, 350);

        let config: RepasteConfig = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.window_ms, DEFAULT_WINDOW_MS);
    }
}
