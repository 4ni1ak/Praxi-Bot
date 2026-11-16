# Desktop UI

## Real-app smoke run

`pnpm --dir apps/desktop/ui smoke` drives the REAL UI (vite dev, not mock mode) against a REAL
local `bot serve` that talks to a scripted OpenAI-compatible stub: 15 flows (first run, bots,
model picker, chat, tools + approval, edit, settings, apps, add-server), a screenshot per step,
exit code 1 on any failure. About 3 minutes after the first build.

- Needs Chrome (`CHROME_BIN` to override) and a Rust toolchain; builds `bot` into `CARGO_TARGET_DIR`
  (default `D:/praxi-target-shared`); `--no-build` reuses it. `--only=<text>` filters steps.
- Starts its own processes (stub, daemon, vite, Chrome) and stops them by PID; temp data dir is deleted.
- Tauri commands are shimmed for the browser (list printed at the end); screenshots go to `SMOKE_SHOTS`.
- Not in CI yet. To add: a job after the UI job running `cargo build -p cli` + `pnpm --dir apps/desktop/ui smoke`
  with `SMOKE_SHOTS` uploaded as an artifact (needs Chrome, preinstalled on GitHub runners).
- It cannot cover the Tauri shell, tray, real CLIs or a real host.
