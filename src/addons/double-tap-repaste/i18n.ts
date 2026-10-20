import i18n from "@/i18n";

// Runtime namespace: keeps upstream's locale files untouched and check:translations green.
// English only; other languages fall back to `en` via fallbackLng.
export const DOUBLE_TAP_REPASTE_NS = "doubleTapRepaste";

const en = {
  label: "Double-tap to re-paste",
  description:
    "While Handy is idle, tap the transcribe shortcut twice quickly to paste the last transcript again. The first tap's brief recording is discarded. Press to start and press to stop work as before. Useful when a paste did not land.",
  window: {
    label: "Double-tap window",
    description:
      "How soon after a quick first tap (under 200 ms) the second press must land to count as a double-tap. Only a tap from idle can start one, so the press that stops a recording never re-pastes.",
  },
};

i18n.addResourceBundle("en", DOUBLE_TAP_REPASTE_NS, en, true, false);
