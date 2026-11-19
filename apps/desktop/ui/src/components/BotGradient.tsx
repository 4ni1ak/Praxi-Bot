import { type CSSProperties } from "react";
import { isLightColor } from "ui";
import { gradientVars } from "../lib/botGradient";
import styles from "./BotGradient.module.css";

/**
 * The bot's grainy mesh gradient: three radial gradients from the bot colour and two hue-shifted
 * derivatives, a slow drift and a fractal-noise grain (design v7, "Bilgisayar kartı"). It fills
 * its positioned parent; `still` freezes the drift (paused / offline cards).
 */
export function BotGradient({ color, still = false }: { color: string | undefined; still?: boolean }) {
  // The palette's own background decides the guard band (read at render: a theme change re-renders the panel).
  const bg = typeof document === "undefined" ? "" : getComputedStyle(document.documentElement).getPropertyValue("--color-background").trim();
  const vars = gradientVars(color, /^#[0-9a-f]{6}$/i.test(bg) && isLightColor(bg)) as CSSProperties;
  return <div className={styles.root} style={vars} data-still={still ? "true" : undefined} data-testid="bot-gradient" aria-hidden="true" />;
}
