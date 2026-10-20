import { useCallback, useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

// Not part of AppSettings: that would regenerate bindings.ts. Rust persists
// this in addons_store.json and the hook talks to it over the event bus.
export interface RepasteConfig {
  enabled: boolean;
  window_ms: number;
}

const EVENT_GET = "handy-addon:double-tap-repaste:get";
const EVENT_SET = "handy-addon:double-tap-repaste:set";
const EVENT_STATE = "handy-addon:double-tap-repaste:state";

export const DEFAULT_WINDOW_MS = 350;
export const MIN_WINDOW_MS = 150;
export const MAX_WINDOW_MS = 600;

// Must match the Rust `Default` so the UI is right before the backend answers.
const DEFAULT_CONFIG: RepasteConfig = {
  enabled: true,
  window_ms: DEFAULT_WINDOW_MS,
};

// Module-level so remounts reuse the last value and share one listener.
let cached: RepasteConfig = DEFAULT_CONFIG;
let ready = false;
let subscribed = false;
// Rust echoes can arrive out of order while dragging the slider; ignore them
// briefly after a local write. Lossless because the client clamps like Rust.
let lastLocalWriteAt = 0;
const ECHO_QUIET_MS = 500;
const subscribers = new Set<(config: RepasteConfig, ready: boolean) => void>();

function clamp(config: RepasteConfig): RepasteConfig {
  const window_ms = Number.isFinite(config.window_ms)
    ? Math.min(
        MAX_WINDOW_MS,
        Math.max(MIN_WINDOW_MS, Math.round(config.window_ms)),
      )
    : DEFAULT_WINDOW_MS;
  return { enabled: config.enabled, window_ms };
}

function publish() {
  subscribers.forEach((notify) => notify(cached, ready));
}

async function subscribe() {
  if (subscribed) return;
  subscribed = true;

  // If the backend never answers, fall back to defaults instead of a
  // permanent spinner.
  const giveUp = setTimeout(() => {
    if (!ready) {
      ready = true;
      publish();
    }
  }, 1500);

  try {
    // Listener first, then ask, or the reply can beat the listener.
    await listen<RepasteConfig>(EVENT_STATE, (event) => {
      clearTimeout(giveUp);
      const settled = Date.now() - lastLocalWriteAt > ECHO_QUIET_MS;
      // Always accept the first reply; afterwards wait out the quiet period.
      if (!ready || settled) {
        cached = clamp(event.payload);
      }
      ready = true;
      publish();
    });
    await emit(EVENT_GET);
  } catch (error) {
    clearTimeout(giveUp);
    console.warn("double-tap-repaste: backend unavailable", error);
    ready = true;
    subscribed = false;
    publish();
  }
}

export function useDoubleTapRepaste() {
  const [state, setState] = useState<{
    config: RepasteConfig;
    ready: boolean;
  }>({
    config: cached,
    ready,
  });

  useEffect(() => {
    const notify = (config: RepasteConfig, isReady: boolean) =>
      setState({ config, ready: isReady });
    subscribers.add(notify);
    void subscribe();
    return () => {
      subscribers.delete(notify);
    };
  }, []);

  const update = useCallback((patch: Partial<RepasteConfig>) => {
    // Clamped to Rust's range so the echo never disagrees with the screen.
    const next = clamp({ ...cached, ...patch });
    cached = next;
    lastLocalWriteAt = Date.now();
    publish();
    emit(EVENT_SET, next).catch((error) => {
      console.warn("double-tap-repaste: failed to apply config", error);
    });
  }, []);

  const setEnabled = useCallback(
    (enabled: boolean) => update({ enabled }),
    [update],
  );
  const setWindowMs = useCallback(
    (window_ms: number) => update({ window_ms }),
    [update],
  );

  return {
    enabled: state.config.enabled,
    windowMs: state.config.window_ms,
    ready: state.ready,
    setEnabled,
    setWindowMs,
  };
}
