import { useCallback, useState } from "react";
import { applyCustomText, type CustomRole, type CustomText } from "../lib/botEditor";
import type { BotDraft } from "../lib/botProfile";

const ROLES: readonly CustomRole[] = ["executor", "planner", "classifier"];

/**
 * The hand-typed model id fields (Advanced) keep their text in the dialog, not in the field, so
 * an unusable id stays visible, blocks Save and counts as unsaved data, and a section change
 * does not lose it.  `setDraft` is the dialog's own draft setter.
 */
export function useCustomText(setDraft: (update: (d: BotDraft) => BotDraft) => void) {
  const [customText, setCustomText] = useState<CustomText>({});

  /** A model picked elsewhere replaces what was typed for that role. */
  const forgetFor = useCallback((patch: Partial<BotDraft>) => {
    const roles = ROLES.filter((r) => r in patch);
    if (roles.length === 0) return;
    setCustomText((c) => {
      const next = { ...c };
      for (const r of roles) delete next[r];
      return next;
    });
  }, []);

  const setCustom = useCallback(
    (role: CustomRole, text: string) => {
      const { ref, keep } = applyCustomText(text);
      if (ref) setDraft((d) => ({ ...d, [role]: ref }));
      setCustomText((c) => {
        const next = { ...c };
        if (keep === undefined) delete next[role];
        else next[role] = keep;
        return next;
      });
    },
    [setDraft]
  );

  const reset = useCallback(() => setCustomText({}), []);
  return { customText, setCustom, forgetFor, reset };
}
