import type { AdminProvider, AdminProviderInput, Bot, BotInput } from "../api/types.ts";

/** What the settings surfaces need from "the place the bots live". */
export interface HomeClient {
  adminPutProvider(id: string, input: AdminProviderInput): Promise<AdminProvider>;
  adminSetDefault(id: string, model?: string): Promise<AdminProvider>;
  createBot(input: BotInput): Promise<Bot>;
  updateBot(id: string, input: BotInput): Promise<Bot>;
}

/** The existing local way of changing providers (native CLI through the app). */
export interface LocalProviders {
  saveProvider(id: string, input: { apiKey?: string; baseUrl?: string; model?: string }): Promise<void>;
  setDefault(id: string, model?: string): Promise<void>;
}

export interface HomeInput {
  /** Client of THIS computer's daemon. */
  local: Pick<HomeClient, "createBot" | "updateBot">;
  localProviders: LocalProviders;
  /** Client connected to the adopted host, when the owner made it the home and it is reachable. */
  host: HomeClient | null;
}

export type HomeTarget = "host" | "local";

/**
 * One place that decides where a settings or bot write goes: the bots' home
 * (the adopted host when there is one) or this computer. Surfaces call these
 * methods and never need to know which it is. Reads stay with the view's own
 * client. Nothing here deletes anything on the other side: the local setup
 * stays as a fallback.
 */
export function botsHome(input: HomeInput) {
  const target: HomeTarget = input.host ? "host" : "local";
  return {
    target,
    async saveProvider(
      id: string,
      p: { apiKey?: string; baseUrl?: string; model?: string; makeDefault?: boolean }
    ) {
      if (input.host) {
        await input.host.adminPutProvider(id, {
          api_key: p.apiKey,
          base_url: p.baseUrl,
          model: p.model,
          make_default: p.makeDefault ?? false
        });
        return;
      }
      await input.localProviders.saveProvider(id, p);
      if (p.makeDefault) {
        await input.localProviders.setDefault(id, p.model);
      }
    },
    async setDefaultModel(id: string, model?: string) {
      if (input.host) {
        await input.host.adminSetDefault(id, model);
      } else {
        await input.localProviders.setDefault(id, model);
      }
    },
    createBot(bot: BotInput): Promise<Bot> {
      return (input.host ?? input.local).createBot(bot);
    },
    updateBot(id: string, bot: BotInput): Promise<Bot> {
      return (input.host ?? input.local).updateBot(id, bot);
    }
  };
}
