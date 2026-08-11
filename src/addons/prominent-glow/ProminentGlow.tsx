import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/ui/ToggleSwitch";
import { Slider } from "../../components/ui/Slider";
import { PROMINENT_GLOW_NS } from "./i18n";
import {
  DEFAULT_INTENSITY,
  MAX_INTENSITY,
  MIN_INTENSITY,
  useProminentGlow,
} from "./useProminentGlow";

interface ProminentGlowProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Settings rows for the Prominent recording glow. Shaped exactly like an
 * upstream boolean setting (ShowTrayIcon, MuteWhileRecording) so it reads as
 * native inside the Advanced > App group, but backed by the addon's own hook
 * rather than `useSettings`.
 */
export const ProminentGlow: React.FC<ProminentGlowProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation(PROMINENT_GLOW_NS);
    const { enabled, intensity, ready, setEnabled, setIntensity } =
      useProminentGlow();

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
            value={intensity}
            onChange={setIntensity}
            min={MIN_INTENSITY}
            max={MAX_INTENSITY}
            step={0.05}
            label={t("intensity.label")}
            description={t("intensity.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
            formatValue={(value) => `${Math.round(value * 100)}%`}
            onReset={() => setIntensity(DEFAULT_INTENSITY)}
          />
        )}
      </>
    );
  },
);
