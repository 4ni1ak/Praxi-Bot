# Translation glossary

Canonical UI terminology for **English (`en`)**, **Turkish (`tr`)**, and **German (`de`)** in the desktop app (`apps/desktop/ui/src/i18n/locales/`). Follow this glossary when adding or updating strings so the interface stays consistent across views.

See [Adding a language](adding-a-language.md) for the setup steps and `check:i18n` workflow.

## Tone and style

| Locale | Address register | Capitalization | Typography |
| --- | --- | --- | --- |
| `en` | Direct, plain ("you") | Sentence case | Curly quotes `“…”`, ellipsis `…`, em-dash `—` |
| `tr` | Warm, polite `sen` (no mixing with `siz`) | Sentence case (no English Title Case) | Ellipsis `…`, em-dash `—`, no apostrophe on common nouns (`botun`, not `bot'un`; keep apostrophe on product name `Praxi-Bot'un`) |
| `de` | Informal `du` / `dein` (no mixing with `Sie`) | Normal German orthography (nouns and sentence starts capitalized) | German quotes `„…“`, spaced ellipsis ` …` after full words, en-dash `–` |

## Core terms

| English (`en`) | Turkish (`tr`) | German (`de`) | Notes |
| --- | --- | --- | --- |
| AI | Yapay zekâ (`yapay zekâ`) | KI | Always use circumflex `â` in Turkish; lowercase mid-sentence. |
| Approval / Approve | onay / Onayla | Genehmigung / Genehmigen | Avoid `Freigabe` in German for approval cards. |
| Allow / Deny | İzin ver / Reddet | Erlauben / Ablehnen | In German, use `Verweigern` only for the permission level (`botForm.level.deny`), `Ablehnen` for denying an approval card. |
| Bot | bot (`Bot` at sentence start) | Bot (`die Bots`) | Common noun in Turkish: `botun`, `bota`, `botu` (no apostrophe). |
| Cancel | İptal | Abbrechen | Keep button labels short and uniform. |
| Chat / Conversation | sohbet | Chat / Unterhaltung | Use `Chat` for the UI item in German; `Unterhaltung` for prose "conversation". |
| Daemon / local service | daemon / yerel servis / arka plan servisi | Daemon / lokaler Dienst / Hintergrunddienst | Use `servis` (not `hizmet`) consistently in Turkish. |
| Hand back / I'm done | Geri ver / Bitirdim | Zurückgeben / Ich bin fertig | Never leave Turkish `Bitirdim` in `en` or `de`. |
| Host / Server | sunucu (`ana makine` only for *host mode*; `ana bilgisayar` only for *host computer*) | Host / Server | Match English distinction between `Host` and `Server` in German. |
| Keychain / Secret store | sistem anahtarlığı / gizli bilgi deposu | Schlüsselbund / Schlüsselspeicher | Avoid literal `sır` (tr) and `Geheimnis`/`Geheimnisspeicher` (de). |
| Main bot | ana bot (badge: `Ana`) | Haupt-Bot (badge: `Haupt`) | Hyphenated in German. |
| Memory (bot feature) | hafıza | Gedächtnis (item: `Erinnerung`) | Reserve Turkish `bellek` for RAM/system memory. |
| Pause / Resume | Duraklat / Devam et | Pausieren / Fortsetzen | Avoid `Sürdür` in Turkish for resume buttons. |
| Prompt | istem | Prompt | — |
| Retry / Try again | Yeniden dene | Erneut versuchen | Prefer `Yeniden dene` over `Tekrar dene` in Turkish. |
| Run (bot execution) | çalışma | Lauf (`Läufe`) | Never use `koşu` or `çalıştırma` for a run in Turkish; never use `Auftrag` in German. |
| Scheduled task / Schedule | zamanlanmış görev / zamanlama | geplante Aufgabe / Zeitplan | — |
| Secret (credential) | gizli bilgi / gizli anahtar | Zugangsdaten / Secret | Use `Zugangsdaten` for general app/API credentials in German. |
| Skill | beceri | Skill (`Skills`) | Kept as `Skill` in German. |
| Sub-agent / Subagent | alt ajan | Subagent (`Subagenten`) | — |
| Take control / Takeover | Kontrolü devral (`Devral`) / devralma | Kontrolle übernehmen / Übernahme | Consistent with `Kontrolle zurückgeben` in German. |
| Token (LLM) | token | Token (`Tokens`) | Avoid `jeton` in Turkish. |
| Turn (chat turn) | tur | Durchgang | Never use `Zug` in German. |
| Workflow | iş akışı | Workflow | Always `iş akışı` (not bare `akış`) in Turkish. |

## Placeholders and structure

- Keep keys, nesting, array types (such as `firstRun.freedom.levels.*.examples`), and `{{placeholder}}` names identical across all locales.
  - Exception: Turkish `chat.step.mcp_read.*` and `chat.step.mcp_search.*` use `{{appAt}}`, supplied by `trLocative()` in [`toolLabels.ts`](../../apps/desktop/ui/src/lib/toolLabels.ts) for vowel harmony and consonant hardening.
- Run `pnpm --dir apps/desktop/ui sort:i18n` and `pnpm --dir apps/desktop/ui check:i18n` before opening a pull request.
