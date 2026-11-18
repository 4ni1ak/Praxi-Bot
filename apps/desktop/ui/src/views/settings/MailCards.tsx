import type { TFunction } from "i18next";
import { EmailCard } from "./EmailCard";
import type { CardDef } from "./types";

/** Cards of the "Mail" section: the mailboxes bots can read and write from. */
export function mailCards(t: TFunction): CardDef[] {
  return [
    {
      id: "email",
      section: "mail",
      kind: "block",
      title: t("settingsModal.email.title"),
      description: t("settingsModal.email.description"),
      keywords: "mail e-posta eposta imap smtp gmail outlook yahoo icloud mailbox posta kutusu agentmail postfach",
      node: <EmailCard />
    }
  ];
}
