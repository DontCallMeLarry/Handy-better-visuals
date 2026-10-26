import i18n from "@/i18n";

// Runtime namespace: keeps upstream's locale files untouched and check:translations green.
// English only; other languages fall back to `en` via fallbackLng.
export const REPASTE_CHORD_NS = "repasteChord";

const en = {
  label: "Paste again shortcut",
  description:
    "Hold Left Command, then press Right Command, to paste the last transcript again. Keep holding Left Command and tap again to paste again. Nothing records. This works when the transcribe shortcut is one side of a modifier. Hold the other side first. macOS only.",
};

i18n.addResourceBundle("en", REPASTE_CHORD_NS, en, true, false);
