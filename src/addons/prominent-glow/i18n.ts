import i18n from "@/i18n";

/**
 * The addon registers its own i18next namespace at runtime instead of adding
 * keys to the per-locale files under `src/i18n/locales`.
 *
 * Two reasons, both about staying out of upstream's way:
 *  - `en/translation.json` is one of the most frequently edited files upstream,
 *    so every added key is a likely merge conflict.
 *  - CI runs `bun run check:translations`, which fails if a key exists in `en`
 *    but is missing from any of the other 23 locales. An en-only addition would
 *    break the build; a runtime namespace is invisible to that check.
 *
 * English only, on purpose — inventing 23 translations would be worse than
 * letting i18next fall back to `en` (which `fallbackLng: "en"` already does).
 */
export const PROMINENT_GLOW_NS = "prominentGlow";

const en = {
  label: "Prominent",
  description:
    "While recording, wash every connected display with an orange glow from all four edges instead of relying on the small overlay. Click-through, so it never blocks what is underneath.",
  intensity: {
    label: "Glow intensity",
    description:
      "How strong the orange is at the screen edge. The centre of the screen is never tinted at any setting.",
  },
};

i18n.addResourceBundle("en", PROMINENT_GLOW_NS, en, true, false);
