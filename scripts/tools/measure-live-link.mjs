#!/usr/bin/env node
// Measures the link between this machine and a Praxi host for the live screen view:
// direct or relayed, round-trip time, frame throughput and capture-to-receive latency.
// No dependencies (Node 18+). Read docs/design/live-view-link-measurement.md first.
//
//   node scripts/tools/measure-live-link.mjs --url http://127.0.0.1:PORT --token TOKEN [--seconds 20] [--target ID]
//
// --url/--token: the loopback proxy printed by `bot --remote <ENDPOINT> proxy`, so the
// requests really travel through the tunnel. Against a local daemon /v1/peer/link is null
// and only the HTTP round trip is measured.
const MEDIA = "application/vnd.praxi.live-view.v1";
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith("--")) acc.push([a.slice(2), all[i + 1]]);
    return acc;
  }, [])
);
const base = (args.url ?? "").replace(/\/$/, "");
const token = args.token ?? process.env.BOT_API_TOKEN ?? "";
const seconds = Number(args.seconds ?? 20);
if (!base || !token) {
  console.error("usage: measure-live-link.mjs --url <base> --token <token> [--seconds 20] [--target <id>]");
  process.exit(2);
}
const auth = { authorization: `Bearer ${token}` };
const api = async (method, path, body, extra = {}) => {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { ...auth, ...(body ? { "content-type": "application/json" } : {}), ...extra },
    body: body ? JSON.stringify(body) : undefined
  });
  return res;
};
const pct = (xs, p) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor((xs.length * p) / 100))] : NaN);
const fmt = (n, d = 0) => (Number.isFinite(n) ? n.toFixed(d) : "n/a");

// 1. Path (direct / relay) and the transport's own RTT estimate, five samples.
const links = [];
for (let i = 0; i < 5; i++) {
  const res = await api("GET", "/v1/peer/link");
  links.push(await res.json());
  await new Promise((r) => setTimeout(r, 400));
}
const real = links.filter(Boolean);
console.log(
  real.length
    ? `path: ${real.every((l) => l.relay) ? "RELAY" : real.some((l) => l.relay) ? "MIXED" : "DIRECT"}, transport rtt ms: ${real.map((l) => l.rtt_ms).join(" ")}`
    : "path: local HTTP (no tunnel: /v1/peer/link is null)"
);

// 2. HTTP round trip of a tiny request (what an ack or an input batch costs).
const rtts = [];
for (let i = 0; i < 20; i++) {
  const t = performance.now();
  await (await api("GET", "/v1/view/status")).arrayBuffer();
  rtts.push(performance.now() - t);
}
console.log(`http round trip ms: p50 ${fmt(pct(rtts, 50))} p95 ${fmt(pct(rtts, 95))} max ${fmt(Math.max(...rtts))}`);

// 3. A live-view watch session: frames/s, bytes/s and capture-to-receive latency.
const targets = await (await api("GET", "/v1/view/targets")).json();
const target = targets.find((t) => (args.target ? t.target_id === args.target : !t.unavailable_reason && t.capabilities.view));
if (!target) {
  console.log("no viewable screen target: skipping the stream measurement");
  process.exit(0);
}
const created = await api("POST", "/v1/view/sessions", {
  client_id: crypto.randomUUID(),
  target_id: target.target_id,
  profile: "watch",
  codecs: ["jpeg"],
  max_width: 1920,
  max_height: 1080
});
if (!created.ok) {
  console.log(`could not open a session: ${created.status} ${await created.text()}`);
  process.exit(1);
}
const session = await created.json();
const id = session.session_id;
const stream = await api("GET", `/v1/view/sessions/${id}/frames`, null, { accept: MEDIA });
const frames = [];
const started = performance.now();
let bytes = 0;
let buf = Buffer.alloc(0);
const deadline = started + seconds * 1000;
const ack = async (seq) => api("POST", `/v1/view/sessions/${id}/ack`, { last_rendered_frame_seq: seq }).catch(() => null);
const reader = stream.body.getReader();
const heartbeat = setInterval(() => api("PUT", `/v1/view/sessions/${id}/heartbeat`, {}).catch(() => null), 4000);
while (performance.now() < deadline) {
  const { done, value } = await reader.read();
  if (done) break;
  bytes += value.length;
  buf = Buffer.concat([buf, value]);
  while (buf.length >= 8) {
    const h = buf.readUInt32BE(0);
    const p = buf.readUInt32BE(4);
    if (buf.length < 8 + h + p) break;
    const header = JSON.parse(buf.subarray(8, 8 + h).toString("utf8"));
    buf = buf.subarray(8 + h + p);
    if (header.type === "frame") {
      const now = Date.now();
      frames.push({ at: performance.now(), size: 8 + h + p, seq: header.frame_seq, latency: now - header.captured_at_ms, send: now - header.sent_at_ms, cause: header.cause });
      void ack(header.frame_seq);
    }
  }
}
clearInterval(heartbeat);
await reader.cancel().catch(() => null);
await api("DELETE", `/v1/view/sessions/${id}`).catch(() => null);

const secs = (performance.now() - started) / 1000;
const sizes = frames.map((f) => f.size);
console.log(`stream: ${frames.length} frames in ${fmt(secs, 1)} s = ${fmt(frames.length / secs, 1)} frames/s, ${fmt(bytes / secs / 1024)} KiB/s`);
console.log(`frame size KiB: p50 ${fmt(pct(sizes, 50) / 1024)} p95 ${fmt(pct(sizes, 95) / 1024)}`);
const lat = frames.map((f) => f.latency);
console.log(`capture-to-receive ms: p50 ${fmt(pct(lat, 50))} p95 ${fmt(pct(lat, 95))} (needs both clocks within ~50 ms; a same-machine host is exact)`);
console.log(`host-send-to-receive ms: p50 ${fmt(pct(frames.map((f) => f.send), 50))} p95 ${fmt(pct(frames.map((f) => f.send), 95))}`);
console.log("tip: change something on the target screen during the run to measure motion frames");
