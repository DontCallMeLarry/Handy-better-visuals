import { useCallback, useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

// Not part of AppSettings: that would regenerate bindings.ts. Rust persists
// this in addons_store.json and the hook talks to it over the event bus.
export interface RepasteConfig {
  enabled: boolean;
}

const EVENT_GET = "handy-addon:repaste-chord:get";
const EVENT_SET = "handy-addon:repaste-chord:set";
const EVENT_STATE = "handy-addon:repaste-chord:state";

// Must match the Rust `Default` so the UI is right before the backend answers.
const DEFAULT_CONFIG: RepasteConfig = { enabled: true };

// Module-level so remounts reuse the last value and share one listener.
let cached: RepasteConfig = DEFAULT_CONFIG;
let ready = false;
let subscribed = false;
const subscribers = new Set<(config: RepasteConfig, ready: boolean) => void>();

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
      cached = { enabled: event.payload.enabled };
      ready = true;
      publish();
    });
    await emit(EVENT_GET);
  } catch (error) {
    clearTimeout(giveUp);
    console.warn("repaste-chord: backend unavailable", error);
    ready = true;
    subscribed = false;
    publish();
  }
}

export function useRepasteChord() {
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

  const setEnabled = useCallback((enabled: boolean) => {
    cached = { enabled };
    publish();
    emit(EVENT_SET, cached).catch((error) => {
      console.warn("repaste-chord: failed to apply config", error);
    });
  }, []);

  return {
    enabled: state.config.enabled,
    ready: state.ready,
    setEnabled,
  };
}
