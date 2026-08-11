import { useCallback, useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

/**
 * Config for the Prominent recording glow.
 *
 * Deliberately NOT part of `AppSettings`:
 *  - `useSettings().getSetting` is typed `keyof AppSettings`, so an addon key
 *    would be a `tsc` error, and `updateSetting` would silently fall into its
 *    "no handler for setting" branch and never persist.
 *  - Adding a field to `AppSettings` regenerates `src/bindings.ts`, a checked-in
 *    1100-line generated file that upstream also rewrites — the single largest
 *    merge-conflict source in this repo.
 *
 * So the addon owns its own state, persisted by Rust in `addons_store.json`,
 * and talks over the event bus rather than through Tauri commands (which would
 * have to be added to `lib.rs`'s `collect_commands!`, regenerating bindings
 * again).
 */
export interface GlowConfig {
  enabled: boolean;
  intensity: number;
}

const EVENT_GET = "handy-addon:prominent-glow:get";
const EVENT_SET = "handy-addon:prominent-glow:set";
const EVENT_STATE = "handy-addon:prominent-glow:state";

export const DEFAULT_INTENSITY = 0.55;
export const MIN_INTENSITY = 0.15;
export const MAX_INTENSITY = 1;

const DEFAULT_CONFIG: GlowConfig = {
  enabled: false,
  intensity: DEFAULT_INTENSITY,
};

// Module-level so a remount (switching settings tabs) reuses the last known
// value instead of flashing defaults, and so N mounted components share one
// listener.
let cached: GlowConfig = DEFAULT_CONFIG;
let ready = false;
let subscribed = false;
/**
 * When the last local write happened.
 *
 * Rust echoes its sanitized config back on every write, and those echoes are
 * dispatched on a multi-threaded async runtime, so two writes issued a few
 * milliseconds apart (i.e. dragging the slider) can have their echoes arrive
 * out of order and rewind the controlled input. Ignoring echoes for a moment
 * after a local write is lossless here because the client clamps to the same
 * range Rust does, so an echo can never carry a correction we don't already
 * have.
 */
let lastLocalWriteAt = 0;
const ECHO_QUIET_MS = 500;
const subscribers = new Set<(config: GlowConfig, ready: boolean) => void>();

function clamp(config: GlowConfig): GlowConfig {
  const intensity = Number.isFinite(config.intensity)
    ? Math.min(MAX_INTENSITY, Math.max(MIN_INTENSITY, config.intensity))
    : DEFAULT_INTENSITY;
  return { enabled: config.enabled, intensity };
}

function publish() {
  subscribers.forEach((notify) => notify(cached, ready));
}

async function subscribe() {
  if (subscribed) return;
  subscribed = true;

  // If the backend never answers — the Rust side of the addon was dropped in an
  // upstream merge, say — stop showing a pending spinner and fall back to
  // defaults rather than leaving the row disabled forever.
  const giveUp = setTimeout(() => {
    if (!ready) {
      ready = true;
      publish();
    }
  }, 1500);

  try {
    // Listener first, then ask — otherwise the reply can beat the listener.
    await listen<GlowConfig>(EVENT_STATE, (event) => {
      clearTimeout(giveUp);
      const settled = Date.now() - lastLocalWriteAt > ECHO_QUIET_MS;
      // Always accept the very first reply (the answer to EVENT_GET); after
      // that, let a settling period pass so a reordered echo cannot rewind a
      // slider the user is still dragging.
      if (!ready || settled) {
        cached = clamp(event.payload);
      }
      ready = true;
      publish();
    });
    await emit(EVENT_GET);
  } catch (error) {
    clearTimeout(giveUp);
    console.warn("prominent-glow: backend unavailable", error);
    ready = true;
    subscribed = false;
    publish();
  }
}

export function useProminentGlow() {
  const [state, setState] = useState<{ config: GlowConfig; ready: boolean }>({
    config: cached,
    ready,
  });

  useEffect(() => {
    const notify = (config: GlowConfig, isReady: boolean) =>
      setState({ config, ready: isReady });
    subscribers.add(notify);
    void subscribe();
    return () => {
      subscribers.delete(notify);
    };
  }, []);

  const update = useCallback((patch: Partial<GlowConfig>) => {
    // Clamped client-side to the same range Rust enforces, so the echo can
    // never disagree with what is already on screen.
    const next = clamp({ ...cached, ...patch });
    cached = next;
    lastLocalWriteAt = Date.now();
    publish();
    emit(EVENT_SET, next).catch((error) => {
      console.warn("prominent-glow: failed to apply config", error);
    });
  }, []);

  const setEnabled = useCallback(
    (enabled: boolean) => update({ enabled }),
    [update],
  );
  const setIntensity = useCallback(
    (intensity: number) => update({ intensity }),
    [update],
  );

  return {
    enabled: state.config.enabled,
    intensity: state.config.intensity,
    ready: state.ready,
    setEnabled,
    setIntensity,
  };
}
