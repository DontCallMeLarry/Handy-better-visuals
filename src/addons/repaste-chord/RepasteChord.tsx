import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/ui/ToggleSwitch";
import { REPASTE_CHORD_NS } from "./i18n";
import { useRepasteChord } from "./useRepasteChord";

interface RepasteChordProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

// A settings row shaped like an upstream boolean setting, backed by the
// addon's own hook rather than useSettings.
export const RepasteChord: React.FC<RepasteChordProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation(REPASTE_CHORD_NS);
    const { enabled, ready, setEnabled } = useRepasteChord();

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={setEnabled}
        isUpdating={!ready}
        label={t("label")}
        description={t("description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);
