import type { BotShapeId } from "ui";
import { BotIdentityPicker } from "../../components/BotIdentityPicker";
import styles from "./FirstRun.module.css";

export interface BotLook {
  name: string;
  shape: BotShapeId;
  color: string;
}

/** The last decision: who the bot is. The same picker as the New-bot modal. */
export function BotStep({
  look,
  recap,
  error,
  onLook
}: {
  look: BotLook;
  recap: string;
  error: string | null;
  onLook: (patch: Partial<BotLook>) => void;
}) {
  return (
    <div className={styles.stack}>
      <BotIdentityPicker
        previewId="first-run-bot"
        name={look.name}
        shape={look.shape}
        color={look.color}
        onName={(name) => onLook({ name })}
        onShape={(shape) => onLook({ shape })}
        onColor={(color) => onLook({ color })}
      />
      <p className={styles.recap}>{recap}</p>
      {error ? (
        <p className={styles.statusBad} role="alert" data-testid="fr-error">
          {error}
        </p>
      ) : null}
    </div>
  );
}
