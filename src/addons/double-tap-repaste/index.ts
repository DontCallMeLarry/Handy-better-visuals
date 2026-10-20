// Importing the component pulls in ./i18n, which registers the addon's
// i18next namespace as a side effect. Keep this barrel as the only entry point
// so the namespace can never be missing at render time.
export { DoubleTapRepaste } from "./DoubleTapRepaste";
export { useDoubleTapRepaste } from "./useDoubleTapRepaste";
export type { RepasteConfig } from "./useDoubleTapRepaste";
