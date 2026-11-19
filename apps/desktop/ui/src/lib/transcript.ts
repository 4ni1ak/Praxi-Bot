/** A chat as a Markdown document (the `/export` command): who said what, and when. */
export interface TranscriptMessage {
  role: string;
  content: string;
  created_at_ms: number;
}

export interface TranscriptLabels {
  title: string;
  user: string;
  bot: string;
}

function stamp(ms: number): string {
  return new Date(ms).toISOString().replace("T", " ").slice(0, 19) + " UTC";
}

export function transcriptMarkdown(messages: readonly TranscriptMessage[], labels: TranscriptLabels): string {
  const parts = [`# ${labels.title}`];
  for (const m of messages) {
    if ((m.role !== "user" && m.role !== "assistant") || !m.content.trim()) {
      continue;
    }
    parts.push(`## ${m.role === "user" ? labels.user : labels.bot} · ${stamp(m.created_at_ms)}\n\n${m.content.trim()}`);
  }
  return `${parts.join("\n\n")}\n`;
}

/** A file name safe on every OS, from the chat's title. */
export function transcriptFileName(title: string): string {
  const base = title.replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-+|-+$/g, "").slice(0, 48);
  return `${base || "chat"}.md`;
}
