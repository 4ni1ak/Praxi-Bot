# Measuring the live-view link (relay vs direct)

Status: measurement guide. Purpose: let the owner decide, with numbers, whether opening a UDP port on the
host (or running a self-hosted relay) is worth it for the live screen view.

## Background
Paired devices talk over iroh (QUIC). The connection starts on a relay and upgrades to a direct
path (hole punching) when the network allows. `GET /v1/peer/link` answers, for the tunnel that carried the
request, `{ "relay": bool, "rtt_ms": n }` (the transport's own RTT estimate; `null` on local HTTP). The live view
ships frames as JPEG records over the same tunnel, so the path decides how many frames per second and how sharp
the picture is while moving. Opening a UDP port only helps when the measurement below shows a RELAY path (or a
slow direct one) on the network the viewer really uses.

## What you need
- The host running `bot serve --peer` (or the desktop app on the host) and a paired viewer device.
- Node 18+ on the viewer machine (no packages).
- Something visible on the host screen, ideally moving (a scrolling page, a video) for part of the run.

## Steps
1. On the viewer machine open the tunnel and keep it open:
   ```sh
   bot --remote <ENDPOINT> proxy
   ```
   It waits until the tunnel is connected, then prints `url:` and `token:` of a loopback endpoint that forwards to
   the host through the tunnel. **Security:** that token grants desktop-role access to the paired host on every route
   (including `/v1/secrets`), not just the measurement calls. While the proxy runs, anyone on this machine who holds the token
   has full desktop-level access to the host. Do not share or log it, and close the proxy with Ctrl-C as soon as you are done.
2. In a second terminal run the probe (20 seconds by default). Give the token through the `BOT_API_TOKEN` environment
   variable, not `--token <token>`: a command-line argument is visible in `ps` and stays in your shell history. The
   script only accepts a loopback `--url` and refuses anything else.
   ```sh
   BOT_API_TOKEN=<token> node scripts/tools/measure-live-link.mjs --url <url> --seconds 30
   ```
   (In PowerShell: `$env:BOT_API_TOKEN = "<token>"` first; clear it afterwards.)
3. Repeat in each network situation you care about: same LAN, phone hotspot, office/guest Wi-Fi, home behind CGNAT.
   Note which one you ran in. The desktop viewer shows the same direct/relay badge, so you can cross-check.

## What it prints
| line | meaning | rule of thumb |
|---|---|---|
| `path:` | DIRECT / RELAY / MIXED over five samples, with the transport RTT | RELAY on a network where direct should work means hole punching fails |
| `http round trip ms` | a tiny request through the tunnel (what an ack or input batch costs) | direct LAN < 20, internet direct 30-80, relay 80-250 |
| `stream: ... frames/s ... KiB/s` | delivered frame rate and bytes (watch profile, 1920x1080 max) | moving content: direct 5+ fps, relay 1-2 fps |
| `frame size KiB` | typical JPEG size | 90-150 KiB for motion, larger for the sharp still frame |
| `capture-to-receive ms` | host capture time to arrival on the viewer | needs clocks within ~50 ms; exact on the same machine |
| `host-send-to-receive ms` | one-way transit of a frame, same clock caveat | relay p50 above ~300 ms feels laggy |

## Deciding
- DIRECT everywhere you use it, p50 transit below ~150 ms: do nothing.
- RELAY with RTT above ~100 ms or fewer than 2 fps on motion, on networks you use often: consider allowing inbound UDP
  for the host's QUIC socket (the port is chosen by the endpoint today; a fixed-port setting would be a follow-up)
  or running a relay closer to you. Opening a port is the owner's call: it widens the host's exposure. Re-run the script afterwards: the path line should turn DIRECT and the fps rise.
- Relay but acceptable numbers: the sharp still frame after motion stops hides most of the loss; leave it.

The probe opens one `watch` session on the first viewable screen (`--target <id>` picks another), acknowledges each frame the
way the app does, and deletes the session at the end. It reads only the screen stream; it sends no input.
