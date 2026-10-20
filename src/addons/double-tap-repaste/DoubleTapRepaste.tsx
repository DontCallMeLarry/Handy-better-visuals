import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/ui/ToggleSwitch";
import { Slider } from "../../components/ui/Slider";
import { DOUBLE_TAP_REPASTE_NS } from "./i18n";
import {
  DEFAULT_WINDOW_MS,
  MAX_WINDOW_MS,
  MIN_WINDOW_MS,
  useDoubleTapRepaste,
} from "./useDoubleTapRepaste";

interface DoubleTapRepasteProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

// Settings rows shaped like an upstream boolean setting, backed by the
// addon's own hook rather than useSettings.
export const DoubleTapRepaste: React.FC<DoubleTapRepasteProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation(DOUBLE_TAP_REPASTE_NS);
    const { enabled, windowMs, ready, setEnabled, setWindowMs } =
      useDoubleTapRepaste();

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={setEnabled}
          isUpdating={!ready}
          label={t("label")}
          description={t("description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />

        {enabled && (
          <Slider
            value={windowMs}
            onChange={setWindowMs}
            min={MIN_WINDOW_MS}
            max={MAX_WINDOW_MS}
            step={50}
            label={t("window.label")}
            description={t("window.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
            formatValue={(value) => `${value}ms`}
            onReset={() => setWindowMs(DEFAULT_WINDOW_MS)}
          />
        )}
      </>
    );
  },
);
